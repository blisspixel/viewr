//! Source-bound Trash, permanent delete, and exact-receipt restore.
//!
//! Default deletes use the platform recycle bin via the `trash` crate (never a
//! raw unlink). Permanent delete is opt-in and should only run after an explicit
//! confirmation in the UI.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Record of a successful trash operation, enough to attempt undo.
#[derive(Debug, Clone)]
pub struct TrashedFile {
    /// Platform receipt identifying the original and trashed locations.
    pub receipt: TrashReceipt,
    /// Playlist index at the time of delete (for restore placement).
    pub playlist_index: usize,
}

/// The durable-in-process information required to undo a trash operation.
#[derive(Clone)]
pub struct TrashReceipt {
    original_path: PathBuf,
    /// macOS does not expose a trash listing API, so preserve the exact URL
    /// returned by `NSFileManager`.
    trashed_path: Option<PathBuf>,
    /// Exact Windows or Freedesktop Trash item identifier captured after moving.
    ///
    /// This remains in process memory and is never logged or persisted. Restore
    /// never falls back to matching only the original pathname.
    platform_id: Option<OsString>,
    /// Live identity handle for the exact object moved to Trash.
    ///
    /// Keeping the handle open prevents identifier reuse while this receipt owns
    /// in-app restore. The handle and its native identity are never exposed.
    restore_source: Option<Arc<crate::fs::ImageSource>>,
    /// Fixed, path-private evidence describing exact-receipt capture.
    capture_status: TrashReceiptCaptureStatus,
}

impl std::fmt::Debug for TrashReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrashReceipt")
            .field("has_platform_id", &self.platform_id.is_some())
            .field("has_trashed_path", &self.trashed_path.is_some())
            .field("has_restore_source", &self.restore_source.is_some())
            .field("capture_status", &self.capture_status.category())
            .finish_non_exhaustive()
    }
}

impl TrashReceipt {
    /// Path the item occupied before it was moved to trash.
    #[must_use]
    pub fn original_path(&self) -> &Path {
        &self.original_path
    }

    /// Whether this receipt identifies one exact item for in-app restore.
    #[must_use]
    pub(crate) fn can_restore_in_app(&self) -> bool {
        if self.restore_source.is_none() {
            return false;
        }

        #[cfg(target_os = "macos")]
        {
            self.trashed_path.is_some()
        }

        #[cfg(any(
            target_os = "windows",
            all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            )
        ))]
        {
            self.platform_id.is_some()
        }

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            )
        )))]
        {
            false
        }
    }

    #[must_use]
    pub(crate) const fn capture_status(&self) -> TrashReceiptCaptureStatus {
        self.capture_status
    }

    /// Retained handle for the exact object named by a successful restore
    /// receipt. This handle stays valid across the Trash round trip.
    #[must_use]
    pub(crate) fn restore_source(&self) -> Option<&crate::fs::ImageSource> {
        self.restore_source.as_deref()
    }

    /// Reopen the restored pathname only when it still names the receipt's
    /// exact retained object, refreshing version evidence after the rename.
    pub(crate) fn open_restored_source(&self) -> Option<crate::fs::ImageSource> {
        self.restore_source
            .as_deref()?
            .reopen_current_regular(&self.original_path)
            .ok()
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        original_path: PathBuf,
        restore_source: Arc<crate::fs::ImageSource>,
    ) -> Self {
        Self {
            trashed_path: Some(original_path.clone()),
            platform_id: Some(OsString::from("test-trash-item")),
            original_path,
            restore_source: Some(restore_source),
            capture_status: TrashReceiptCaptureStatus::Bound,
        }
    }

    #[cfg(test)]
    pub(crate) fn without_restore_for_test(original_path: PathBuf) -> Self {
        Self {
            original_path,
            trashed_path: None,
            platform_id: None,
            restore_source: None,
            capture_status: TrashReceiptCaptureStatus::NoCandidate,
        }
    }
}

/// Fixed, path-private result of attempting to bind a native Trash receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    target_os = "macos",
    allow(
        dead_code,
        reason = "shared diagnostic categories include Windows and Linux listing outcomes"
    )
)]
pub(crate) enum TrashReceiptCaptureStatus {
    Bound,
    /// Historical category for a pre-move listing failure. Capture no longer
    /// pre-lists Trash, but restore and diagnostic code still understand it.
    #[allow(dead_code)] // retained so older capture evidence categories stay complete
    PreListFailed,
    PostListFailed,
    NoCandidate,
    AmbiguousCandidate,
    IdentityMismatch,
    Unsupported,
    NotAttempted,
}

impl TrashReceiptCaptureStatus {
    #[must_use]
    pub(crate) const fn category(self) -> &'static str {
        match self {
            Self::Bound => "bound",
            Self::PreListFailed => "pre_list_failed",
            Self::PostListFailed => "post_list_failed",
            Self::NoCandidate => "no_candidate",
            Self::AmbiguousCandidate => "ambiguous_candidate",
            Self::IdentityMismatch => "identity_mismatch",
            Self::Unsupported => "unsupported",
            Self::NotAttempted => "not_attempted",
        }
    }
}

/// Fixed restore failure evidence and its safe next action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    target_os = "macos",
    allow(
        dead_code,
        reason = "shared recovery copy includes Windows/Linux-only failure categories"
    )
)]
pub(crate) enum TrashRestoreError {
    DestinationOccupied,
    AccessDenied,
    OperationFailed,
    MissingFromTrash,
    AmbiguousReceipt,
    Unsupported,
    InvalidReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrashRestoreDisposition {
    RetryNow,
    ResolveThenRetry,
    ManualReview,
    Terminal,
}

impl TrashRestoreError {
    #[must_use]
    pub(crate) const fn disposition(self) -> TrashRestoreDisposition {
        match self {
            Self::OperationFailed => TrashRestoreDisposition::RetryNow,
            Self::DestinationOccupied | Self::AccessDenied => {
                TrashRestoreDisposition::ResolveThenRetry
            }
            Self::AmbiguousReceipt | Self::Unsupported | Self::InvalidReceipt => {
                TrashRestoreDisposition::ManualReview
            }
            Self::MissingFromTrash => TrashRestoreDisposition::Terminal,
        }
    }

    #[must_use]
    pub(crate) const fn category(self) -> &'static str {
        match self {
            Self::DestinationOccupied => "destination_occupied",
            Self::AccessDenied => "access_denied",
            Self::OperationFailed => "operation_failed",
            Self::MissingFromTrash => "exact_item_missing",
            Self::AmbiguousReceipt => "ambiguous_receipt",
            Self::Unsupported => "unsupported",
            Self::InvalidReceipt => "invalid_receipt",
        }
    }

    #[must_use]
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::DestinationOccupied => {
                "the original folder already contains an item with that name"
            }
            Self::AccessDenied => "restore access was denied",
            Self::OperationFailed => "the operating system could not restore the file",
            Self::MissingFromTrash => "the exact item is no longer in the system Trash",
            Self::AmbiguousReceipt => "the exact Trash receipt is ambiguous",
            Self::Unsupported => "in-app restore is unsupported on this platform",
            Self::InvalidReceipt => "the exact Trash receipt is unavailable",
        }
    }
}

#[derive(Debug)]
pub(crate) struct TrashRestoreFailure {
    pub(crate) record: TrashedFile,
    pub(crate) error: TrashRestoreError,
}

pub(crate) struct TrashRestoreOutcome {
    pub(crate) restored: Vec<TrashedFile>,
    pub(crate) failures: Vec<TrashRestoreFailure>,
}

impl TrashRestoreOutcome {
    #[must_use]
    pub(crate) fn failure_count(&self, disposition: TrashRestoreDisposition) -> usize {
        self.failures
            .iter()
            .filter(|failure| failure.error.disposition() == disposition)
            .count()
    }

    #[must_use]
    pub(crate) fn first_failure(&self) -> Option<TrashRestoreError> {
        self.failures.first().map(|failure| failure.error)
    }

    #[must_use]
    pub(crate) fn restored_playlist_index(&self, original_index: usize) -> usize {
        let missing_before = self
            .failures
            .iter()
            .filter(|failure| failure.record.playlist_index < original_index)
            .count();
        original_index.saturating_sub(missing_before)
    }

    pub(crate) fn take_retryable_records(&mut self) -> Vec<TrashedFile> {
        let failures = std::mem::take(&mut self.failures);
        let omitted_indices = failures
            .iter()
            .filter(|failure| {
                matches!(
                    failure.error.disposition(),
                    TrashRestoreDisposition::ManualReview | TrashRestoreDisposition::Terminal
                )
            })
            .map(|failure| failure.record.playlist_index)
            .collect::<Vec<_>>();
        let mut retryable = failures
            .into_iter()
            .filter(|failure| {
                matches!(
                    failure.error.disposition(),
                    TrashRestoreDisposition::RetryNow | TrashRestoreDisposition::ResolveThenRetry
                )
            })
            .map(|failure| failure.record)
            .collect::<Vec<_>>();
        rebase_trashed_file_indices(&mut retryable, &omitted_indices);
        retryable
    }
}

pub(crate) fn rebase_trashed_file_indices(records: &mut [TrashedFile], omitted_indices: &[usize]) {
    for record in records {
        let omitted_before = omitted_indices
            .iter()
            .filter(|index| **index < record.playlist_index)
            .count();
        record.playlist_index = record.playlist_index.saturating_sub(omitted_before);
    }
}

pub(crate) fn rebase_trashed_file_indices_after_current_removals(
    records: &mut [TrashedFile],
    removed_current_indices: &[usize],
) {
    let pending_indices = records
        .iter()
        .map(|record| record.playlist_index)
        .collect::<Vec<_>>();
    for record in records {
        let pending_before = pending_indices
            .iter()
            .filter(|index| **index < record.playlist_index)
            .count();
        let current_gap = record.playlist_index.saturating_sub(pending_before);
        let removed_before = removed_current_indices
            .iter()
            .filter(|index| **index < current_gap)
            .count();
        record.playlist_index = record.playlist_index.saturating_sub(removed_before);
    }
}

/// Why a source-bound destructive action did not reach its filesystem sink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuardedActionError {
    /// The pathname no longer identifies the accepted presentation source.
    Changed,
    /// The accepted source has disappeared from its pathname.
    Missing,
    /// The source or current entry is a link, reparse point, or non-regular object.
    Unsupported,
    /// The operating system would not provide trustworthy identity evidence.
    Unavailable,
    /// Source identity matched, but the platform operation failed.
    OperationFailed(String),
}

impl GuardedActionError {
    /// Fixed path-private category for operator diagnostics.
    #[must_use]
    pub(crate) fn category(&self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
            Self::Unavailable => "identity_unavailable",
            Self::OperationFailed(_) => "operation_failed",
        }
    }
}

/// After removing `removed` paths from `files`, return the index to show next.
///
/// Prefers the slot that previously held `old_index` (the image that "took the
/// place" of a deleted one). Clamps when the list shrinks past the end.
#[must_use]
/// Choose the catalog index to present after `removed` paths leave the playlist.
///
/// `old_index` is the pre-removal catalog index of the deleted selection. Callers
/// that already moved selection elsewhere should not use this helper to force
/// navigation; they should keep the surviving selection path instead.
pub fn index_after_removals(files: &[PathBuf], old_index: usize, removed: &[PathBuf]) -> usize {
    if files.is_empty() {
        return 0;
    }
    // Post-removal list only: clamping lands on the image that filled the deleted
    // slot, or the new last image when the deleted selection was the end.
    let _ = removed;
    old_index.min(files.len().saturating_sub(1))
}

/// Remove every path in `to_remove` from `files`, preserving relative order.
///
/// Returns the paths that were actually present and removed.
pub fn remove_from_playlist(files: &mut Vec<PathBuf>, to_remove: &[PathBuf]) -> Vec<PathBuf> {
    let kill: HashSet<&Path> = to_remove.iter().map(PathBuf::as_path).collect();
    let mut removed = Vec::new();
    files.retain(|p| {
        if kill.contains(p.as_path()) {
            removed.push(p.clone());
            false
        } else {
            true
        }
    });
    removed
}

/// Move `path` to the system trash / recycle bin.
///
/// # Errors
/// Returns a human-readable reason if the platform trash API fails.
pub fn move_to_trash(path: &Path) -> Result<TrashReceipt, String> {
    let source = Arc::new(
        crate::fs::ImageSource::open(path)
            .map_err(|error| file_access_failure(error.kind()).to_owned())?,
    );
    source_bound_trash_with(
        path,
        &source,
        TrashReceiptCapture::new,
        |path, source, _receipt_capture| {
            let mut receipt = move_to_trash_unbound(path, source)?;
            TrashReceiptCapture::bind_receipts(std::iter::once(&mut receipt));
            Ok(receipt)
        },
    )
    .map_err(|error| match error {
        GuardedActionError::OperationFailed(message) => message,
        other => other.category().to_owned(),
    })
}

fn move_to_trash_unbound(
    path: &Path,
    restore_source: Arc<crate::fs::ImageSource>,
) -> Result<TrashReceipt, String> {
    let original_path = crate::fs::canonical_existing_file_path(path)
        .map_err(|error| file_access_failure(error.kind()).to_owned())?;

    #[cfg(target_os = "macos")]
    let (trashed_path, capture_status) = {
        let trashed_path = crate::macos::move_to_trash(&original_path)?;
        if restore_source.same_object_at_path(&trashed_path) {
            (Some(trashed_path), TrashReceiptCaptureStatus::Bound)
        } else {
            (None, TrashReceiptCaptureStatus::IdentityMismatch)
        }
    };

    #[cfg(not(target_os = "macos"))]
    let trashed_path = {
        trash::delete(&original_path).map_err(|error| trash_error_message(&error))?;
        None
    };

    #[cfg(not(target_os = "macos"))]
    let capture_status = if cfg!(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    )) {
        TrashReceiptCaptureStatus::NotAttempted
    } else {
        TrashReceiptCaptureStatus::Unsupported
    };

    Ok(TrashReceipt {
        original_path,
        trashed_path,
        platform_id: None,
        restore_source: Some(restore_source),
        capture_status,
    })
}

/// Captures exact Trash receipts after a move without a pre-move full listing.
///
/// Listing the whole Recycle Bin or free-desktop trash twice made every Delete
/// pay shell enumeration cost proportional to trash size. Capture now does one
/// post-move list and binds by original path plus retained object identity.
struct TrashReceiptCapture;

impl TrashReceiptCapture {
    const fn new() -> Self {
        Self
    }

    fn bind_receipts<'a>(receipts: impl IntoIterator<Item = &'a mut TrashReceipt>) {
        #[cfg(any(
            target_os = "windows",
            all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            )
        ))]
        {
            Self::bind_receipts_with(receipts, trash::os_limited::list);
        }

        #[cfg(not(any(
            target_os = "windows",
            all(
                unix,
                not(target_os = "macos"),
                not(target_os = "ios"),
                not(target_os = "android")
            )
        )))]
        {
            for _ in receipts {}
        }
    }

    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    fn bind_receipts_with<'a, E>(
        receipts: impl IntoIterator<Item = &'a mut TrashReceipt>,
        list: impl FnOnce() -> Result<Vec<trash::TrashItem>, E>,
    ) {
        let mut receipts = receipts.into_iter().peekable();
        if receipts.peek().is_none() {
            return;
        }

        let items = list();
        let Ok(items) = items else {
            for receipt in receipts {
                receipt.platform_id = None;
                receipt.capture_status = TrashReceiptCaptureStatus::PostListFailed;
            }
            return;
        };

        for receipt in receipts {
            match exact_trash_item_id_for_receipt(&items, receipt) {
                Ok(platform_id) => {
                    receipt.platform_id = Some(platform_id);
                    receipt.capture_status = TrashReceiptCaptureStatus::Bound;
                }
                Err(status) => {
                    receipt.platform_id = None;
                    receipt.capture_status = status;
                }
            }
        }
    }
}

fn source_bound_trash_with<T>(
    path: &Path,
    source: &Arc<crate::fs::ImageSource>,
    prepare: impl FnOnce() -> T,
    trash: impl FnOnce(&Path, Arc<crate::fs::ImageSource>, T) -> Result<TrashReceipt, String>,
) -> Result<TrashReceipt, GuardedActionError> {
    let prepared = prepare();
    verify_accepted_source(path, source)?;
    trash(path, Arc::clone(source), prepared).map_err(GuardedActionError::OperationFailed)
}

/// Move the current entry to Trash only if it is still the accepted source.
///
/// # Errors
/// Returns a fixed source-rejection category or the path-private platform error.
pub(crate) fn move_source_to_trash(
    path: &Path,
    source: &Arc<crate::fs::ImageSource>,
) -> Result<TrashReceipt, GuardedActionError> {
    source_bound_trash_with(
        path,
        source,
        TrashReceiptCapture::new,
        |path, source, _receipt_capture| {
            let mut receipt = move_to_trash_unbound(path, source)?;
            TrashReceiptCapture::bind_receipts(std::iter::once(&mut receipt));
            Ok(receipt)
        },
    )
}

/// Permanently delete `path` (not recoverable via OS trash).
///
/// Callers must obtain explicit user confirmation first.
///
/// # Errors
/// Returns a human-readable reason if the filesystem remove fails.
pub fn permanent_delete(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|error| file_access_failure(error.kind()).to_owned())
}

/// Permanently delete the current entry only if it is still the accepted source.
///
/// Callers must obtain explicit user confirmation first. A separate pre-confirmation
/// call to [`verify_accepted_source_native`] avoids presenting a stale destructive
/// prompt without reading the whole file on the event loop; this function performs
/// the full content check after confirmation immediately before deletion.
///
/// # Errors
/// Returns a fixed source-rejection category or the path-private filesystem error.
pub(crate) fn permanent_delete_source(
    path: &Path,
    source: &crate::fs::ImageSource,
) -> Result<(), GuardedActionError> {
    guarded_source_action_with(path, source, permanent_delete)
}

/// Verify that `path` still identifies the source which supplied accepted pixels.
///
/// # Errors
/// Returns a fixed category when object identity cannot be proven unchanged.
pub(crate) fn verify_accepted_source(
    path: &Path,
    source: &crate::fs::ImageSource,
) -> Result<(), GuardedActionError> {
    classify_source_match(source.matches_path(path))
}

/// Perform the bounded native-only check used before presenting a destructive
/// confirmation. The post-confirmation worker repeats the full content check.
pub(crate) fn verify_accepted_source_native(
    path: &Path,
    source: &crate::fs::ImageSource,
) -> Result<(), GuardedActionError> {
    classify_source_match(source.matches_path_native(path))
}

fn classify_source_match(
    source_match: crate::fs::ImageSourceMatch,
) -> Result<(), GuardedActionError> {
    match source_match {
        crate::fs::ImageSourceMatch::Same => Ok(()),
        crate::fs::ImageSourceMatch::Changed => Err(GuardedActionError::Changed),
        crate::fs::ImageSourceMatch::Missing => Err(GuardedActionError::Missing),
        crate::fs::ImageSourceMatch::Unsupported => Err(GuardedActionError::Unsupported),
        crate::fs::ImageSourceMatch::Unavailable => Err(GuardedActionError::Unavailable),
    }
}

fn guarded_source_action_with<T>(
    path: &Path,
    source: &crate::fs::ImageSource,
    action: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, GuardedActionError> {
    verify_accepted_source(path, source)?;
    action(path).map_err(GuardedActionError::OperationFailed)
}

/// Best-effort restore of a previously trashed path from the OS trash.
///
/// # Errors
/// Returns an error if the trash cannot be listed, the item is gone, or restore fails.
pub fn restore_from_trash(receipt: &TrashReceipt) -> Result<(), String> {
    restore_from_trash_platform(receipt).map_err(|error| error.message().to_owned())
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
pub(crate) fn restore_trash_batch(records: Vec<TrashedFile>) -> TrashRestoreOutcome {
    restore_trash_batch_with_shared_state(
        records,
        || trash::os_limited::list().map_err(|error| trash_restore_error(&error)),
        restore_from_trash_snapshot,
    )
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
pub(crate) fn restore_trash_batch(records: Vec<TrashedFile>) -> TrashRestoreOutcome {
    restore_trash_batch_with(records, restore_from_trash_platform)
}

fn restore_trash_batch_with(
    records: Vec<TrashedFile>,
    mut restore: impl FnMut(&TrashReceipt) -> Result<(), TrashRestoreError>,
) -> TrashRestoreOutcome {
    let mut restored = Vec::new();
    let mut failures = Vec::new();
    for record in records {
        match restore(&record.receipt) {
            Ok(()) => restored.push(record),
            Err(error) => {
                failures.push(TrashRestoreFailure { record, error });
            }
        }
    }
    TrashRestoreOutcome { restored, failures }
}

#[cfg(any(
    test,
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore_trash_batch_with_shared_state<S>(
    records: Vec<TrashedFile>,
    prepare: impl FnOnce() -> Result<S, TrashRestoreError>,
    mut restore: impl FnMut(&TrashReceipt, &mut S) -> Result<(), TrashRestoreError>,
) -> TrashRestoreOutcome {
    let mut shared = match prepare() {
        Ok(shared) => shared,
        Err(error) => {
            return TrashRestoreOutcome {
                restored: Vec::new(),
                failures: records
                    .into_iter()
                    .map(|record| TrashRestoreFailure { record, error })
                    .collect(),
            };
        }
    };
    restore_trash_batch_with(records, |receipt| restore(receipt, &mut shared))
}

#[cfg(target_os = "macos")]
fn restore_from_trash_platform(receipt: &TrashReceipt) -> Result<(), TrashRestoreError> {
    let trashed_path = receipt
        .trashed_path
        .as_deref()
        .ok_or(TrashRestoreError::InvalidReceipt)?;
    let restore_source = receipt
        .restore_source
        .as_deref()
        .ok_or(TrashRestoreError::InvalidReceipt)?;
    if !restore_source.same_object_at_path(trashed_path) {
        return Err(TrashRestoreError::InvalidReceipt);
    }
    crate::macos::restore_from_trash(trashed_path, &receipt.original_path)
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore_from_trash_platform(receipt: &TrashReceipt) -> Result<(), TrashRestoreError> {
    let mut items = trash::os_limited::list().map_err(|error| trash_restore_error(&error))?;
    restore_from_trash_snapshot(receipt, &mut items)
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore_from_trash_snapshot(
    receipt: &TrashReceipt,
    items: &mut Vec<trash::TrashItem>,
) -> Result<(), TrashRestoreError> {
    let expected_id = receipt
        .platform_id
        .as_ref()
        .ok_or(TrashRestoreError::InvalidReceipt)?;
    let restore_source = receipt
        .restore_source
        .as_deref()
        .ok_or(TrashRestoreError::InvalidReceipt)?;
    let item = take_exact_trash_item(items, expected_id, &receipt.original_path)?;
    if !trash_item_matches_source(&item, restore_source) {
        return Err(TrashRestoreError::InvalidReceipt);
    }
    trash::os_limited::restore_all([item]).map_err(|error| trash_restore_error(&error))
}

#[cfg(all(
    test,
    any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    )
))]
fn exact_trash_item(
    mut items: Vec<trash::TrashItem>,
    expected_id: &OsString,
    original_path: &Path,
) -> Result<trash::TrashItem, TrashRestoreError> {
    take_exact_trash_item(&mut items, expected_id, original_path)
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn take_exact_trash_item(
    items: &mut Vec<trash::TrashItem>,
    expected_id: &OsString,
    original_path: &Path,
) -> Result<trash::TrashItem, TrashRestoreError> {
    let index = {
        let mut matching = items
            .iter()
            .enumerate()
            .filter(|(_, item)| &item.id == expected_id);
        let (index, item) = matching.next().ok_or(TrashRestoreError::MissingFromTrash)?;
        if matching.next().is_some() {
            return Err(TrashRestoreError::AmbiguousReceipt);
        }
        if !same_trash_origin(original_path, &item.original_path()) {
            return Err(TrashRestoreError::InvalidReceipt);
        }
        index
    };
    Ok(items.swap_remove(index))
}

#[cfg(not(target_os = "macos"))]
fn trash_error_message(error: &trash::Error) -> String {
    match error {
        trash::Error::Unknown { .. } => "the system Trash operation failed".to_owned(),
        trash::Error::Os { code, .. } => {
            let kind = std::io::Error::from_raw_os_error(*code).kind();
            file_access_failure(kind).to_owned()
        }
        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        trash::Error::FileSystem { source, .. } => file_access_failure(source.kind()).to_owned(),
        trash::Error::TargetedRoot => "system root items cannot be moved to Trash".to_owned(),
        trash::Error::CouldNotAccess { .. } | trash::Error::CanonicalizePath { .. } => {
            "the file is unavailable or access was denied".to_owned()
        }
        trash::Error::ConvertOsString { .. } => {
            "the file name is not supported by system Trash".to_owned()
        }
        trash::Error::RestoreCollision { .. } => {
            "the original folder already contains an item with that name".to_owned()
        }
        trash::Error::RestoreTwins { .. } => {
            "multiple matching Trash items prevent a safe restore".to_owned()
        }
    }
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn trash_restore_error(error: &trash::Error) -> TrashRestoreError {
    match error {
        trash::Error::Os { code, .. } => {
            restore_io_error(std::io::Error::from_raw_os_error(*code).kind())
        }
        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        trash::Error::FileSystem { source, .. } => restore_io_error(source.kind()),
        trash::Error::RestoreCollision { .. } => TrashRestoreError::DestinationOccupied,
        trash::Error::RestoreTwins { .. } => TrashRestoreError::AmbiguousReceipt,
        trash::Error::TargetedRoot | trash::Error::ConvertOsString { .. } => {
            TrashRestoreError::Unsupported
        }
        trash::Error::Unknown { .. }
        | trash::Error::CouldNotAccess { .. }
        | trash::Error::CanonicalizePath { .. } => TrashRestoreError::OperationFailed,
    }
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
const fn restore_io_error(kind: std::io::ErrorKind) -> TrashRestoreError {
    match kind {
        std::io::ErrorKind::PermissionDenied => TrashRestoreError::AccessDenied,
        std::io::ErrorKind::AlreadyExists => TrashRestoreError::DestinationOccupied,
        _ => TrashRestoreError::OperationFailed,
    }
}

pub(crate) const fn file_access_failure(kind: std::io::ErrorKind) -> &'static str {
    match kind {
        std::io::ErrorKind::NotFound => "the file could not be found",
        std::io::ErrorKind::PermissionDenied => "access was denied",
        std::io::ErrorKind::AlreadyExists => {
            "the original folder already contains an item with that name"
        }
        _ => "the operating system rejected the file operation",
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited Win32 ordinal path comparison
fn windows_path_eq_ignore_case(left: &Path, right: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let left = left.as_os_str().encode_wide().collect::<Vec<_>>();
    let right = right.as_os_str().encode_wide().collect::<Vec<_>>();
    let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return false;
    };
    // SAFETY: Both pointers reference initialized UTF-16 buffers for the exact
    // explicit lengths supplied. `CompareStringOrdinal` does not retain them.
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn same_trash_origin(receipt_path: &Path, listed_path: &Path) -> bool {
    crate::fs::canonical_file_path(listed_path).is_ok_and(|path| {
        #[cfg(target_os = "windows")]
        {
            windows_path_eq_ignore_case(receipt_path, &path)
        }

        #[cfg(not(target_os = "windows"))]
        {
            path == receipt_path
        }
    })
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn exact_trash_item_id_for_receipt(
    items: &[trash::TrashItem],
    receipt: &TrashReceipt,
) -> Result<OsString, TrashReceiptCaptureStatus> {
    let restore_source = receipt
        .restore_source
        .as_deref()
        .ok_or(TrashReceiptCaptureStatus::IdentityMismatch)?;
    classify_trash_item_id(items, &receipt.original_path, |item| {
        trash_item_matches_source(item, restore_source)
    })
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn classify_trash_item_id(
    items: &[trash::TrashItem],
    original_path: &Path,
    mut matches_source: impl FnMut(&trash::TrashItem) -> bool,
) -> Result<OsString, TrashReceiptCaptureStatus> {
    let mut saw_same_origin = false;
    let mut captured = None;
    for item in items {
        if !same_trash_origin(original_path, &item.original_path()) {
            continue;
        }
        saw_same_origin = true;
        if !matches_source(item) {
            continue;
        }
        if captured.is_some() {
            return Err(TrashReceiptCaptureStatus::AmbiguousCandidate);
        }
        captured = Some(item.id.clone());
    }
    captured.ok_or(if saw_same_origin {
        TrashReceiptCaptureStatus::IdentityMismatch
    } else {
        TrashReceiptCaptureStatus::NoCandidate
    })
}

#[cfg(target_os = "windows")]
fn trash_item_data_path(item: &trash::TrashItem) -> PathBuf {
    PathBuf::from(&item.id)
}

#[cfg(all(
    unix,
    not(target_os = "macos"),
    not(target_os = "ios"),
    not(target_os = "android")
))]
fn trash_item_data_path(item: &trash::TrashItem) -> PathBuf {
    let info_path = Path::new(&item.id);
    let Some(trash_root) = info_path.parent().and_then(Path::parent) else {
        return PathBuf::new();
    };
    let Some(name_in_trash) = info_path.file_stem() else {
        return PathBuf::new();
    };
    trash_root.join("files").join(name_in_trash)
}

#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn trash_item_matches_source(item: &trash::TrashItem, source: &crate::fs::ImageSource) -> bool {
    source.same_object_at_path(&trash_item_data_path(item))
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
fn restore_from_trash_platform(_receipt: &TrashReceipt) -> Result<(), TrashRestoreError> {
    Err(TrashRestoreError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::{
        GuardedActionError, TrashReceipt, TrashReceiptCaptureStatus, TrashedFile,
        index_after_removals, move_source_to_trash, permanent_delete_source, remove_from_playlist,
        restore_from_trash, restore_trash_batch_with, verify_accepted_source,
    };
    use crate::ephemeral::TempWorkspace;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const RESTORE_STRESS_RECEIPTS: usize = 64;

    #[test]
    fn trashed_file_record_holds_path() {
        let t = TrashedFile {
            receipt: TrashReceipt {
                original_path: PathBuf::from("a.jpg"),
                trashed_path: None,
                platform_id: None,
                restore_source: None,
                capture_status: TrashReceiptCaptureStatus::NoCandidate,
            },
            playlist_index: 3,
        };
        assert_eq!(t.playlist_index, 3);
        assert_eq!(t.receipt.original_path(), PathBuf::from("a.jpg"));
    }

    #[test]
    fn remove_from_playlist_preserves_order_and_index_clamp() {
        let mut files = vec![
            PathBuf::from("a.jpg"),
            PathBuf::from("b.jpg"),
            PathBuf::from("c.jpg"),
            PathBuf::from("d.jpg"),
        ];
        let removed = remove_from_playlist(
            &mut files,
            &[PathBuf::from("b.jpg"), PathBuf::from("d.jpg")],
        );
        assert_eq!(removed.len(), 2);
        assert_eq!(files, vec![PathBuf::from("a.jpg"), PathBuf::from("c.jpg")]);
        assert_eq!(index_after_removals(&files, 3, &removed), 1);
        assert_eq!(index_after_removals(&files, 0, &removed), 0);
        assert_eq!(index_after_removals(&[], 0, &removed), 0);
    }

    #[test]
    fn permanent_delete_removes_file() {
        let ws = TempWorkspace::new("curate_perm").unwrap();
        let path = ws.path().join("gone.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]))
            .save(&path)
            .unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();
        assert!(path.is_file());
        permanent_delete_source(&path, &source).unwrap();
        assert!(!path.is_file());
    }

    #[test]
    fn source_bound_single_trash_rejects_replacement_before_sink() {
        let workspace = TempWorkspace::new("curate_single_trash_replacement").unwrap();
        let path = workspace.path().join("selected.png");
        let original = workspace.path().join("original.png");
        std::fs::write(&path, b"accepted object").unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();
        std::fs::rename(&path, &original).unwrap();
        std::fs::write(&path, b"unreviewed replacement").unwrap();

        let mut sink_calls = 0;
        let error = super::guarded_source_action_with(&path, &source, |_| -> Result<(), String> {
            sink_calls += 1;
            panic!("a replacement must not reach the single Trash sink")
        })
        .unwrap_err();

        assert_eq!(error, GuardedActionError::Changed);
        assert_eq!(sink_calls, 0);
        assert!(path.is_file());
        assert!(original.is_file());
    }

    #[test]
    fn single_trash_revalidates_after_receipt_preparation() {
        let workspace = TempWorkspace::new("curate_trash_receipt_preparation_swap").unwrap();
        let path = workspace.path().join("selected.png");
        let original = workspace.path().join("original.png");
        std::fs::write(&path, b"accepted object").unwrap();
        let source = Arc::new(crate::fs::ImageSource::open(&path).unwrap());

        let mut sink_calls = 0;
        let error = super::source_bound_trash_with(
            &path,
            &source,
            || {
                std::fs::rename(&path, &original).unwrap();
                std::fs::write(&path, b"replacement during receipt preparation").unwrap();
            },
            |_, _, ()| -> Result<TrashReceipt, String> {
                sink_calls += 1;
                panic!("a preparation-window replacement must not reach Trash")
            },
        )
        .unwrap_err();

        assert_eq!(error, GuardedActionError::Changed);
        assert_eq!(sink_calls, 0);
        assert!(path.is_file());
        assert!(original.is_file());
    }

    #[test]
    fn permanent_delete_revalidates_after_the_confirmation_window() {
        let workspace = TempWorkspace::new("curate_permanent_confirmation_swap").unwrap();
        let path = workspace.path().join("selected.png");
        let original = workspace.path().join("original.png");
        std::fs::write(&path, b"accepted object").unwrap();
        let source = crate::fs::ImageSource::open(&path).unwrap();

        verify_accepted_source(&path, &source).expect("pre-confirmation identity is current");
        std::fs::rename(&path, &original).unwrap();
        std::fs::write(&path, b"replacement while confirmation is open").unwrap();

        let mut sink_calls = 0;
        let error = super::guarded_source_action_with(&path, &source, |_| -> Result<(), String> {
            sink_calls += 1;
            panic!("a confirmation-window replacement must not reach remove_file")
        })
        .unwrap_err();

        assert_eq!(error, GuardedActionError::Changed);
        assert_eq!(sink_calls, 0);
        assert!(path.is_file());
        assert!(original.is_file());
    }

    #[test]
    fn guarded_action_reports_fixed_source_and_operation_categories() {
        let workspace = TempWorkspace::new("curate_guarded_categories").unwrap();
        let missing = workspace.path().join("missing.png");
        std::fs::write(&missing, b"accepted then removed").unwrap();
        let missing_source = crate::fs::ImageSource::open(&missing).unwrap();
        std::fs::remove_file(&missing).unwrap();
        assert_eq!(
            verify_accepted_source(&missing, &missing_source),
            Err(GuardedActionError::Missing)
        );

        let unavailable = workspace.path().join("unavailable.png");
        std::fs::write(&unavailable, b"identity intentionally unavailable").unwrap();
        let unavailable_source =
            crate::fs::ImageSource::open_without_identity_for_test(&unavailable).unwrap();
        assert_eq!(
            verify_accepted_source(&unavailable, &unavailable_source),
            Err(GuardedActionError::Unavailable)
        );

        let current = workspace.path().join("current.png");
        std::fs::write(&current, b"same object").unwrap();
        let current_source = crate::fs::ImageSource::open(&current).unwrap();
        let operation_error = super::guarded_source_action_with(&current, &current_source, |_| {
            Err::<(), _>("access denied".to_owned())
        })
        .unwrap_err();
        assert_eq!(
            operation_error,
            GuardedActionError::OperationFailed("access denied".to_owned())
        );
        assert_eq!(GuardedActionError::Changed.category(), "changed");
        assert_eq!(GuardedActionError::Missing.category(), "missing");
        assert_eq!(GuardedActionError::Unsupported.category(), "unsupported");
        assert_eq!(
            GuardedActionError::Unavailable.category(),
            "identity_unavailable"
        );
        assert_eq!(operation_error.category(), "operation_failed");
    }

    #[test]
    fn batch_restore_preserves_only_failed_receipts_for_retry() {
        let records = ["first.jpg", "blocked.jpg", "third.jpg"]
            .into_iter()
            .enumerate()
            .map(|(playlist_index, name)| TrashedFile {
                receipt: TrashReceipt {
                    original_path: PathBuf::from(name),
                    trashed_path: None,
                    platform_id: None,
                    restore_source: None,
                    capture_status: TrashReceiptCaptureStatus::NoCandidate,
                },
                playlist_index,
            })
            .collect();

        let outcome = restore_trash_batch_with(records, |receipt| {
            if receipt.original_path() == Path::new("blocked.jpg") {
                Err(super::TrashRestoreError::DestinationOccupied)
            } else {
                Ok(())
            }
        });

        assert_eq!(outcome.restored.len(), 2);
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(
            outcome.failures[0].record.receipt.original_path(),
            Path::new("blocked.jpg")
        );
        assert_eq!(
            outcome.failures[0].error,
            super::TrashRestoreError::DestinationOccupied
        );
        assert_eq!(
            outcome.failure_count(super::TrashRestoreDisposition::ResolveThenRetry),
            1
        );
        assert_eq!(
            outcome.first_failure(),
            Some(super::TrashRestoreError::DestinationOccupied)
        );
    }

    #[test]
    fn batch_restore_prepares_shared_state_once_for_the_bounded_receipt_set() {
        use std::cell::Cell;

        let records = (0..RESTORE_STRESS_RECEIPTS)
            .map(|playlist_index| TrashedFile {
                receipt: TrashReceipt::without_restore_for_test(PathBuf::from(format!(
                    "image-{playlist_index}.jpg"
                ))),
                playlist_index,
            })
            .collect();
        let preparation_calls = Cell::new(0);
        let restore_calls = Cell::new(0);

        let outcome = super::restore_trash_batch_with_shared_state(
            records,
            || {
                preparation_calls.set(preparation_calls.get() + 1);
                Ok(())
            },
            |_, ()| {
                restore_calls.set(restore_calls.get() + 1);
                Ok(())
            },
        );

        assert_eq!(preparation_calls.get(), 1);
        assert_eq!(restore_calls.get(), RESTORE_STRESS_RECEIPTS);
        assert_eq!(outcome.restored.len(), RESTORE_STRESS_RECEIPTS);
        assert!(outcome.failures.is_empty());
    }

    #[test]
    fn batch_restore_snapshot_failure_preserves_every_receipt_for_retry() {
        use std::cell::Cell;

        let records = (0..RESTORE_STRESS_RECEIPTS)
            .map(|playlist_index| TrashedFile {
                receipt: TrashReceipt::without_restore_for_test(PathBuf::from(format!(
                    "image-{playlist_index}.jpg"
                ))),
                playlist_index,
            })
            .collect();
        let restore_calls = Cell::new(0);

        let mut outcome = super::restore_trash_batch_with_shared_state(
            records,
            || -> Result<(), super::TrashRestoreError> {
                Err(super::TrashRestoreError::OperationFailed)
            },
            |_, ()| {
                restore_calls.set(restore_calls.get() + 1);
                Ok(())
            },
        );

        assert_eq!(restore_calls.get(), 0);
        assert!(outcome.restored.is_empty());
        assert_eq!(outcome.failures.len(), RESTORE_STRESS_RECEIPTS);
        assert_eq!(
            outcome.first_failure(),
            Some(super::TrashRestoreError::OperationFailed)
        );
        assert_eq!(
            outcome.take_retryable_records().len(),
            RESTORE_STRESS_RECEIPTS
        );
    }

    #[test]
    fn partial_restore_indices_account_for_earlier_failures() {
        let failed = |name: &str, playlist_index, error| super::TrashRestoreFailure {
            record: TrashedFile {
                receipt: TrashReceipt {
                    original_path: PathBuf::from(name),
                    trashed_path: None,
                    platform_id: None,
                    restore_source: None,
                    capture_status: TrashReceiptCaptureStatus::NoCandidate,
                },
                playlist_index,
            },
            error,
        };
        let outcome = super::TrashRestoreOutcome {
            restored: Vec::new(),
            failures: vec![
                failed("b.jpg", 1, super::TrashRestoreError::DestinationOccupied),
                failed("d.jpg", 3, super::TrashRestoreError::MissingFromTrash),
            ],
        };

        assert_eq!(outcome.restored_playlist_index(0), 0);
        assert_eq!(outcome.restored_playlist_index(2), 1);
        assert_eq!(outcome.restored_playlist_index(4), 2);
    }

    #[test]
    fn second_restore_attempt_rebases_after_a_terminal_predecessor() {
        let records = ["terminal-a.jpg", "retry-b.jpg", "retry-c.jpg"]
            .into_iter()
            .enumerate()
            .map(|(playlist_index, name)| TrashedFile {
                receipt: TrashReceipt {
                    original_path: PathBuf::from(name),
                    trashed_path: None,
                    platform_id: None,
                    restore_source: None,
                    capture_status: TrashReceiptCaptureStatus::NoCandidate,
                },
                playlist_index,
            })
            .collect();
        let mut first_attempt = restore_trash_batch_with(records, |receipt| {
            match receipt.original_path().to_string_lossy().as_ref() {
                "terminal-a.jpg" => Err(super::TrashRestoreError::MissingFromTrash),
                "retry-b.jpg" => Err(super::TrashRestoreError::OperationFailed),
                _ => Err(super::TrashRestoreError::DestinationOccupied),
            }
        });

        let retryable = first_attempt.take_retryable_records();
        assert_eq!(
            retryable
                .iter()
                .map(|record| record.playlist_index)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );

        let second_attempt = restore_trash_batch_with(retryable, |_| Ok(()));
        assert!(second_attempt.failures.is_empty());
        assert_eq!(
            second_attempt
                .restored
                .iter()
                .map(|record| record.playlist_index)
                .collect::<Vec<_>>(),
            vec![0, 1],
            "terminal receipts cannot leave holes in a later successful retry"
        );
    }

    #[test]
    fn restore_failures_have_exhaustive_safe_dispositions() {
        use super::{TrashRestoreDisposition as Disposition, TrashRestoreError as Error};

        let cases = [
            (
                Error::OperationFailed,
                Disposition::RetryNow,
                "operation_failed",
            ),
            (
                Error::DestinationOccupied,
                Disposition::ResolveThenRetry,
                "destination_occupied",
            ),
            (
                Error::AccessDenied,
                Disposition::ResolveThenRetry,
                "access_denied",
            ),
            (
                Error::AmbiguousReceipt,
                Disposition::ManualReview,
                "ambiguous_receipt",
            ),
            (Error::Unsupported, Disposition::ManualReview, "unsupported"),
            (
                Error::InvalidReceipt,
                Disposition::ManualReview,
                "invalid_receipt",
            ),
            (
                Error::MissingFromTrash,
                Disposition::Terminal,
                "exact_item_missing",
            ),
        ];
        for (error, disposition, category) in cases {
            assert_eq!(error.disposition(), disposition);
            assert_eq!(error.category(), category);
            assert!(!error.message().contains(['\\', '/']));
        }
    }

    #[test]
    fn trash_receipt_debug_never_exposes_paths_or_platform_identifiers() {
        let secret = "C:\\Users\\private\\album\\secret.png";
        let receipt = TrashReceipt {
            original_path: PathBuf::from(secret),
            trashed_path: Some(PathBuf::from(secret)),
            platform_id: Some(std::ffi::OsString::from(secret)),
            restore_source: None,
            capture_status: TrashReceiptCaptureStatus::AmbiguousCandidate,
        };

        let debug = format!("{receipt:?}");
        assert!(debug.contains("has_platform_id: true"));
        assert!(debug.contains("has_trashed_path: true"));
        assert!(debug.contains("has_restore_source: false"));
        assert!(!debug.contains("private"));
        assert!(!debug.contains("secret.png"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn external_trash_errors_never_expose_payloads_or_paths() {
        let secret = "C:\\Users\\private\\album\\secret.png\n\u{202e}gpj";
        let errors = [
            trash::Error::Unknown {
                description: secret.to_owned(),
            },
            trash::Error::Os {
                code: 5,
                description: secret.to_owned(),
            },
            trash::Error::CouldNotAccess {
                target: secret.to_owned(),
            },
            trash::Error::CanonicalizePath {
                original: PathBuf::from(secret),
            },
            trash::Error::ConvertOsString {
                original: std::ffi::OsString::from(secret),
            },
        ];
        for error in errors {
            let message = super::trash_error_message(&error);
            assert!(!message.contains("private"));
            assert!(!message.contains('\n'));
            assert!(!message.contains('\u{202e}'));
            assert!(!message.contains(['\\', '/']));
        }
    }

    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    #[test]
    fn trash_origin_comparison_normalizes_platform_path_spelling() {
        let listed = std::env::current_dir().unwrap().join("Cargo.toml");
        let receipt = crate::fs::canonical_file_path(&listed).unwrap();
        #[cfg(target_os = "windows")]
        {
            let canonical_listed = crate::fs::canonical_file_path(&listed).unwrap();
            assert!(super::windows_path_eq_ignore_case(
                &receipt,
                &canonical_listed
            ));
            assert!(!super::windows_path_eq_ignore_case(
                &receipt,
                &canonical_listed.with_file_name("README.md")
            ));
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert!(super::same_trash_origin(&receipt, &listed));
            assert!(!super::same_trash_origin(
                &receipt,
                &listed.with_file_name("README.md")
            ));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn trash_origin_comparison_uses_windows_case_semantics() {
        let workspace = TempWorkspace::new("trash_case_match").unwrap();
        let actual = workspace.path().join("MiXeDCase.JPG");
        std::fs::write(&actual, b"case probe").unwrap();
        let listed = crate::fs::canonical_file_path(&actual).unwrap();
        let receipt = listed.with_file_name("mixedcase.jpg");

        assert!(super::windows_path_eq_ignore_case(&receipt, &listed));
    }

    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    #[test]
    fn exact_trash_restore_selection_never_falls_back_to_path() {
        let workspace = TempWorkspace::new("exact_trash_selection").unwrap();
        let parent = workspace.path().canonicalize().unwrap();
        let item = |id: &str, name: &str| trash::TrashItem {
            id: id.into(),
            name: name.into(),
            original_parent: parent.clone(),
            time_deleted: 0,
        };
        let original = parent.join("photo.jpg");
        let expected = std::ffi::OsString::from("new");
        let selected = super::exact_trash_item(
            vec![item("old", "photo.jpg"), item("new", "photo.jpg")],
            &expected,
            &original,
        )
        .unwrap();
        assert_eq!(selected.id, expected);

        assert_eq!(
            super::exact_trash_item(
                vec![item("old", "photo.jpg")],
                &std::ffi::OsString::from("new"),
                &original,
            )
            .unwrap_err(),
            super::TrashRestoreError::MissingFromTrash
        );
        assert_eq!(
            super::exact_trash_item(
                vec![item("new", "other.jpg")],
                &std::ffi::OsString::from("new"),
                &original,
            )
            .unwrap_err(),
            super::TrashRestoreError::InvalidReceipt
        );
        assert_eq!(
            super::exact_trash_item(
                vec![item("new", "photo.jpg"), item("new", "photo.jpg")],
                &std::ffi::OsString::from("new"),
                &original,
            )
            .unwrap_err(),
            super::TrashRestoreError::AmbiguousReceipt
        );
    }

    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    #[test]
    fn receipt_capture_uses_one_post_move_listing_and_fails_closed() {
        use std::cell::Cell;

        let mut post_list_receipt = TrashReceipt::without_restore_for_test("post.jpg".into());
        let post_list_calls = Cell::new(0);
        super::TrashReceiptCapture::bind_receipts_with(
            std::iter::once(&mut post_list_receipt),
            || -> Result<Vec<trash::TrashItem>, ()> {
                post_list_calls.set(post_list_calls.get() + 1);
                Err(())
            },
        );
        assert_eq!(
            post_list_receipt.capture_status(),
            TrashReceiptCaptureStatus::PostListFailed
        );
        assert_eq!(post_list_calls.get(), 1);

        let empty_list_calls = Cell::new(0);
        super::TrashReceiptCapture::bind_receipts_with(
            std::iter::empty(),
            || -> Result<Vec<trash::TrashItem>, ()> {
                empty_list_calls.set(empty_list_calls.get() + 1);
                Ok(Vec::new())
            },
        );
        assert_eq!(empty_list_calls.get(), 0);

        let workspace = TempWorkspace::new("receipt_capture_no_candidate").unwrap();
        let path = workspace.path().join("missing.jpg");
        std::fs::write(&path, b"accepted object").unwrap();
        let mut no_candidate = TrashReceipt {
            original_path: path.clone(),
            trashed_path: None,
            platform_id: None,
            restore_source: Some(Arc::new(crate::fs::ImageSource::open(&path).unwrap())),
            capture_status: TrashReceiptCaptureStatus::NotAttempted,
        };
        super::TrashReceiptCapture::bind_receipts_with(
            std::iter::once(&mut no_candidate),
            || -> Result<Vec<trash::TrashItem>, ()> { Ok(Vec::new()) },
        );
        assert_eq!(
            no_candidate.capture_status(),
            TrashReceiptCaptureStatus::NoCandidate
        );
    }

    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    #[test]
    fn receipt_candidate_classification_is_exact_and_fail_closed() {
        let workspace = TempWorkspace::new("receipt_candidate_classification").unwrap();
        let parent = workspace.path().canonicalize().unwrap();
        let original = parent.join("photo.jpg");
        let item = |id: &str| trash::TrashItem {
            id: id.into(),
            name: "photo.jpg".into(),
            original_parent: parent.clone(),
            time_deleted: 0,
        };
        let first = item("first");
        let second = item("second");
        let unrelated = trash::TrashItem {
            id: "other".into(),
            name: "other.jpg".into(),
            original_parent: parent.join("elsewhere"),
            time_deleted: 0,
        };

        assert_eq!(
            super::classify_trash_item_id(&[], &original, |_| true),
            Err(TrashReceiptCaptureStatus::NoCandidate)
        );
        assert_eq!(
            super::classify_trash_item_id(&[unrelated], &original, |_| true),
            Err(TrashReceiptCaptureStatus::NoCandidate),
            "unrelated Trash items cannot satisfy the receipt"
        );
        assert_eq!(
            super::classify_trash_item_id(std::slice::from_ref(&first), &original, |_| false,),
            Err(TrashReceiptCaptureStatus::IdentityMismatch)
        );
        assert_eq!(
            super::classify_trash_item_id(
                &[first.clone(), second.clone()],
                &original,
                |candidate| candidate.id == first.id,
            ),
            Ok(first.id.clone()),
            "one identity-bound candidate remains exact among same-origin items"
        );
        assert_eq!(
            super::classify_trash_item_id(&[first, second], &original, |_| true),
            Err(TrashReceiptCaptureStatus::AmbiguousCandidate)
        );
    }

    #[cfg(any(
        target_os = "windows",
        all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        )
    ))]
    #[test]
    fn receipt_capture_requires_the_moved_source_identity() {
        let workspace = TempWorkspace::new("identity_bound_trash_capture").unwrap();
        let parent = workspace.path().canonicalize().unwrap();
        let original = parent.join("photo.jpg");
        std::fs::write(&original, b"accepted object").unwrap();
        let source = Arc::new(crate::fs::ImageSource::open(&original).unwrap());

        #[cfg(target_os = "windows")]
        let (platform_id, data_path) = {
            let data_path = parent.join("trashed-photo.jpg");
            std::fs::rename(&original, &data_path).unwrap();
            (data_path.as_os_str().to_owned(), data_path)
        };

        #[cfg(all(
            unix,
            not(target_os = "macos"),
            not(target_os = "ios"),
            not(target_os = "android")
        ))]
        let (platform_id, data_path) = {
            let trash_root = parent.join("trash-root");
            let info_dir = trash_root.join("info");
            let files_dir = trash_root.join("files");
            std::fs::create_dir_all(&info_dir).unwrap();
            std::fs::create_dir_all(&files_dir).unwrap();
            let info_path = info_dir.join("trashed-photo.jpg.trashinfo");
            let data_path = files_dir.join("trashed-photo.jpg");
            std::fs::write(&info_path, b"[Trash Info]").unwrap();
            std::fs::rename(&original, &data_path).unwrap();
            (info_path.as_os_str().to_owned(), data_path)
        };

        let item = trash::TrashItem {
            id: platform_id.clone(),
            name: "photo.jpg".into(),
            original_parent: parent.clone(),
            time_deleted: 0,
        };
        let receipt = TrashReceipt {
            original_path: original,
            trashed_path: None,
            platform_id: None,
            restore_source: Some(source),
            capture_status: TrashReceiptCaptureStatus::NotAttempted,
        };
        assert_eq!(
            super::exact_trash_item_id_for_receipt(std::slice::from_ref(&item), &receipt,),
            Ok(platform_id)
        );

        let retained = parent.join("retained-original-object");
        std::fs::rename(&data_path, retained).unwrap();
        std::fs::write(&data_path, b"same-path substitute").unwrap();
        assert_eq!(
            super::exact_trash_item_id_for_receipt(&[item], &receipt),
            Err(TrashReceiptCaptureStatus::IdentityMismatch),
            "a sole same-origin item is not enough without source identity"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mixed_case_windows_trash_receipt_restores() {
        let workspace = TempWorkspace::new("trash_case_roundtrip").unwrap();
        let actual = workspace.path().join("MiXeDCase.JPG");
        std::fs::write(&actual, b"case probe").unwrap();
        let alias = workspace.path().join("mixedcase.jpg");

        let receipt = super::move_to_trash(&alias).unwrap();
        assert_eq!(
            receipt.original_path().file_name().unwrap(),
            std::ffi::OsStr::new("MiXeDCase.JPG")
        );
        assert!(!actual.exists(), "file should leave the folder after trash");
        if receipt.can_restore_in_app() {
            restore_from_trash(&receipt).unwrap();
            assert!(actual.is_file());
        } else {
            assert_ne!(
                receipt.capture_status(),
                TrashReceiptCaptureStatus::Bound,
                "receipt capability and capture status must agree"
            );
            eprintln!(
                "exact Trash receipt unavailable: category={}",
                receipt.capture_status().category()
            );
        }
    }

    #[test]
    fn restore_rejects_a_path_without_a_matching_trash_receipt() {
        let ws = TempWorkspace::new("curate_missing_restore").unwrap();
        let receipt = TrashReceipt {
            original_path: ws.path().join("never-trashed.png"),
            trashed_path: None,
            platform_id: None,
            restore_source: None,
            capture_status: TrashReceiptCaptureStatus::NoCandidate,
        };
        assert!(restore_from_trash(&receipt).is_err());
    }

    #[test]
    fn trash_and_restore_roundtrip() {
        let ws = TempWorkspace::new("curate_roundtrip").unwrap();
        let path = ws.path().join("photo.png");
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]));
        img.save(&path).unwrap();
        assert!(path.is_file());

        let source = Arc::new(crate::fs::ImageSource::open(&path).unwrap());
        match move_source_to_trash(&path, &source) {
            Ok(receipt) => {
                assert!(!path.is_file(), "file should leave the folder after trash");
                if receipt.can_restore_in_app() {
                    restore_from_trash(&receipt).unwrap();
                    assert!(path.is_file(), "restore should put the file back");
                } else {
                    assert_ne!(
                        receipt.capture_status(),
                        TrashReceiptCaptureStatus::Bound,
                        "receipt capability and capture status must agree"
                    );
                    eprintln!(
                        "exact Trash receipt unavailable: category={}",
                        receipt.capture_status().category()
                    );
                }
            }
            Err(GuardedActionError::OperationFailed(_)) => {
                eprintln!("source-bound Trash API unavailable in this environment");
            }
            Err(error) => panic!("current accepted source should pass preflight: {error:?}"),
        }
    }
}
