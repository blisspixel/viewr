//! Curation: flag, trash, permanent delete, and restore.
//!
//! Default deletes use the platform recycle bin via the `trash` crate (never a
//! raw unlink). Permanent delete is opt-in and should only run after an explicit
//! confirmation in the UI. Flag sets support the photographer cull workflow:
//! mark many files, then batch-trash once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Record of a successful trash operation, enough to attempt undo.
#[derive(Debug, Clone)]
pub struct TrashedFile {
    /// Platform receipt identifying the original and trashed locations.
    pub receipt: TrashReceipt,
    /// Playlist index at the time of delete (for restore placement).
    pub playlist_index: usize,
}

/// The durable-in-process information required to undo a trash operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashReceipt {
    original_path: PathBuf,
    /// macOS does not expose a trash listing API, so preserve the exact URL
    /// returned by `NSFileManager`. Other supported desktops restore by the
    /// original path through the `trash` crate.
    trashed_path: Option<PathBuf>,
}

impl TrashReceipt {
    /// Path the item occupied before it was moved to trash.
    #[must_use]
    pub fn original_path(&self) -> &Path {
        &self.original_path
    }
}

pub(crate) struct TrashRestoreOutcome {
    pub(crate) restored: Vec<TrashedFile>,
    pub(crate) failed: Vec<TrashedFile>,
    pub(crate) first_error: Option<String>,
}

/// In-memory set of paths flagged for later batch delete.
#[derive(Debug, Default, Clone)]
pub struct FlagSet {
    paths: HashSet<PathBuf>,
}

impl FlagSet {
    /// Create an empty flag set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of flagged paths.
    #[must_use]
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// True when nothing is flagged.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Whether `path` is currently flagged.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }

    /// Toggle flag on `path`. Returns the new flagged state (`true` = flagged).
    pub fn toggle(&mut self, path: &Path) -> bool {
        if self.paths.remove(path) {
            false
        } else {
            self.paths.insert(path.to_path_buf());
            true
        }
    }

    /// Flag `path` if not already flagged.
    pub fn insert(&mut self, path: PathBuf) {
        self.paths.insert(path);
    }

    /// Unflag `path` if present.
    pub fn remove(&mut self, path: &Path) {
        self.paths.remove(path);
    }

    /// Clear all flags.
    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// Snapshot flagged paths as a sorted list (stable for tests and batch ops).
    #[must_use]
    pub fn paths_sorted(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.paths.iter().cloned().collect();
        v.sort();
        v
    }

    /// Remove and return all flagged paths (sorted).
    pub fn take_all_sorted(&mut self) -> Vec<PathBuf> {
        let v = self.paths_sorted();
        self.paths.clear();
        v
    }
}

/// After removing `removed` paths from `files`, return the index to show next.
///
/// Prefers the slot that previously held `old_index` (the image that "took the
/// place" of a deleted one). Clamps when the list shrinks past the end.
#[must_use]
pub fn index_after_removals(files: &[PathBuf], old_index: usize, removed: &[PathBuf]) -> usize {
    if files.is_empty() {
        return 0;
    }
    let remove: HashSet<&Path> = removed.iter().map(PathBuf::as_path).collect();
    // Count how many removed entries sat strictly before old_index in the
    // pre-removal ordering is unknown here; callers pass the post-removal list.
    // We only clamp the previous index into the new range.
    let _ = remove;
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
    let original_path = crate::fs::canonical_existing_file_path(path)
        .map_err(|error| format!("could not resolve trash path: {error}"))?;

    #[cfg(target_os = "macos")]
    let trashed_path = Some(crate::macos::move_to_trash(&original_path)?);

    #[cfg(not(target_os = "macos"))]
    let trashed_path = {
        trash::delete(&original_path).map_err(|e| e.to_string())?;
        None
    };

    Ok(TrashReceipt {
        original_path,
        trashed_path,
    })
}

/// Permanently delete `path` (not recoverable via OS trash).
///
/// Callers must obtain explicit user confirmation first.
///
/// # Errors
/// Returns a human-readable reason if the filesystem remove fails.
pub fn permanent_delete(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// Best-effort restore of a previously trashed path from the OS trash.
///
/// # Errors
/// Returns an error if the trash cannot be listed, the item is gone, or restore fails.
pub fn restore_from_trash(receipt: &TrashReceipt) -> Result<(), String> {
    restore_from_trash_platform(receipt)
}

pub(crate) fn restore_trash_batch(records: Vec<TrashedFile>) -> TrashRestoreOutcome {
    restore_trash_batch_with(records, restore_from_trash)
}

pub(crate) fn restored_playlist_index(original_index: usize, failed: &[TrashedFile]) -> usize {
    let missing_before = failed
        .iter()
        .filter(|record| record.playlist_index < original_index)
        .count();
    original_index.saturating_sub(missing_before)
}

fn restore_trash_batch_with(
    records: Vec<TrashedFile>,
    mut restore: impl FnMut(&TrashReceipt) -> Result<(), String>,
) -> TrashRestoreOutcome {
    let mut restored = Vec::new();
    let mut failed = Vec::new();
    let mut first_error = None;
    for record in records {
        match restore(&record.receipt) {
            Ok(()) => restored.push(record),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                failed.push(record);
            }
        }
    }
    TrashRestoreOutcome {
        restored,
        failed,
        first_error,
    }
}

#[cfg(target_os = "macos")]
fn restore_from_trash_platform(receipt: &TrashReceipt) -> Result<(), String> {
    let trashed_path = receipt
        .trashed_path
        .as_deref()
        .ok_or("macOS trash receipt is missing the resulting item path")?;
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
fn restore_from_trash_platform(receipt: &TrashReceipt) -> Result<(), String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    let matching = windows_matching_trash_items(items, &receipt.original_path)?;

    #[cfg(not(target_os = "windows"))]
    let matching: Vec<_> = items
        .into_iter()
        .filter(|item| same_trash_origin(&receipt.original_path, &item.original_path()))
        .collect();
    if matching.is_empty() {
        return Err(
            "could not find the file in the system trash (it may have been emptied)".into(),
        );
    }
    trash::os_limited::restore_all(matching).map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn windows_matching_trash_items(
    items: Vec<trash::TrashItem>,
    receipt_path: &Path,
) -> Result<Vec<trash::TrashItem>, String> {
    let mut exact = Vec::new();
    let mut case_fallback = Vec::new();
    for item in items {
        let Ok(listed_path) = crate::fs::canonical_file_path(&item.original_path()) else {
            continue;
        };
        if listed_path == receipt_path {
            exact.push(item);
        } else if windows_path_eq_ignore_case(receipt_path, &listed_path) {
            case_fallback.push(item);
        }
    }
    let match_count = exact.len() + case_fallback.len();
    match match_count {
        0 => Ok(Vec::new()),
        1 if exact.is_empty() => Ok(case_fallback),
        1 => Ok(exact),
        _ => Err(
            "multiple recycle-bin items match this path with different casing; restore is ambiguous"
                .into(),
        ),
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

#[cfg(all(
    unix,
    not(target_os = "macos"),
    not(target_os = "ios"),
    not(target_os = "android")
))]
fn same_trash_origin(receipt_path: &Path, listed_path: &Path) -> bool {
    crate::fs::canonical_file_path(listed_path).is_ok_and(|path| path == receipt_path)
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
fn restore_from_trash_platform(_receipt: &TrashReceipt) -> Result<(), String> {
    Err("in-app trash restore is unsupported on this platform".into())
}

/// Trash many paths, attempting every item and returning both successes and failures.
#[must_use]
pub fn trash_many(paths: &[PathBuf]) -> (Vec<TrashReceipt>, Vec<(PathBuf, String)>) {
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    for path in paths {
        match move_to_trash(path) {
            Ok(receipt) => ok.push(receipt),
            Err(error) => failed.push((path.clone(), error)),
        }
    }
    (ok, failed)
}

#[cfg(test)]
mod tests {
    use super::{
        FlagSet, TrashReceipt, TrashedFile, index_after_removals, move_to_trash, permanent_delete,
        remove_from_playlist, restore_from_trash, restore_trash_batch_with,
        restored_playlist_index, trash_many,
    };
    use crate::ephemeral::TempWorkspace;
    use std::path::{Path, PathBuf};

    #[test]
    fn trashed_file_record_holds_path() {
        let t = TrashedFile {
            receipt: TrashReceipt {
                original_path: PathBuf::from("a.jpg"),
                trashed_path: None,
            },
            playlist_index: 3,
        };
        assert_eq!(t.playlist_index, 3);
        assert_eq!(t.receipt.original_path(), PathBuf::from("a.jpg"));
    }

    #[test]
    fn flag_set_toggle_and_batch() {
        let mut flags = FlagSet::new();
        assert!(flags.is_empty());
        assert!(flags.toggle(PathBuf::from("a.jpg").as_path()));
        assert!(flags.contains(PathBuf::from("a.jpg").as_path()));
        assert!(!flags.toggle(PathBuf::from("a.jpg").as_path()));
        assert!(flags.is_empty());

        flags.insert(PathBuf::from("c.jpg"));
        flags.insert(PathBuf::from("b.jpg"));
        assert_eq!(flags.len(), 2);
        flags.remove(Path::new("b.jpg"));
        assert!(!flags.contains(Path::new("b.jpg")));
        flags.insert(PathBuf::from("b.jpg"));
        let batch = flags.take_all_sorted();
        assert_eq!(batch, vec![PathBuf::from("b.jpg"), PathBuf::from("c.jpg")]);
        assert!(flags.is_empty());

        flags.insert(PathBuf::from("d.jpg"));
        flags.clear();
        assert!(flags.is_empty());
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
        assert!(path.is_file());
        permanent_delete(&path).unwrap();
        assert!(!path.is_file());
    }

    #[test]
    fn trash_many_partial_success() {
        let ws = TempWorkspace::new("curate_many").unwrap();
        let good = ws.path().join("ok.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1]))
            .save(&good)
            .unwrap();
        let missing = ws.path().join("missing.png");
        let (ok, failed) = trash_many(&[good.clone(), missing.clone()]);
        assert_eq!(failed.len(), 1, "the missing input must be reported");
        assert_eq!(failed[0].0, missing);
        // The valid item succeeds when the environment provides a trash API.
        if !ok.is_empty() {
            let canonical_good = crate::fs::canonical_file_path(&good).unwrap();
            assert!(
                ok.iter()
                    .any(|receipt| receipt.original_path() == canonical_good)
            );
            assert!(!good.is_file());
        }
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
                },
                playlist_index,
            })
            .collect();

        let outcome = restore_trash_batch_with(records, |receipt| {
            if receipt.original_path() == Path::new("blocked.jpg") {
                Err("destination occupied".into())
            } else {
                Ok(())
            }
        });

        assert_eq!(outcome.restored.len(), 2);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(
            outcome.failed[0].receipt.original_path(),
            Path::new("blocked.jpg")
        );
        assert_eq!(outcome.first_error.as_deref(), Some("destination occupied"));
    }

    #[test]
    fn partial_restore_indices_account_for_earlier_failures() {
        let failed = vec![TrashedFile {
            receipt: TrashReceipt {
                original_path: PathBuf::from("b.jpg"),
                trashed_path: None,
            },
            playlist_index: 1,
        }];

        assert_eq!(restored_playlist_index(0, &failed), 0);
        assert_eq!(restored_playlist_index(2, &failed), 1);
        assert_eq!(restored_playlist_index(3, &failed), 2);
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

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_trash_selection_rejects_every_case_ambiguous_match() {
        let workspace = TempWorkspace::new("trash_case_selection").unwrap();
        let parent = workspace.path().canonicalize().unwrap();
        let item = |id: &str, name: &str| trash::TrashItem {
            id: id.into(),
            name: name.into(),
            original_parent: parent.clone(),
            time_deleted: 0,
        };
        let exact_receipt = parent.join("photo.jpg");
        let exact =
            super::windows_matching_trash_items(vec![item("lower", "photo.jpg")], &exact_receipt)
                .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].id, std::ffi::OsString::from("lower"));

        let items = vec![item("upper", "Photo.JPG"), item("lower", "photo.jpg")];
        assert!(
            super::windows_matching_trash_items(items.clone(), &exact_receipt)
                .unwrap_err()
                .contains("ambiguous")
        );

        let ambiguous_receipt = parent.join("pHoTo.jpg");
        assert!(
            super::windows_matching_trash_items(items, &ambiguous_receipt)
                .unwrap_err()
                .contains("ambiguous")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn mixed_case_windows_trash_receipt_restores() {
        let workspace = TempWorkspace::new("trash_case_roundtrip").unwrap();
        let actual = workspace.path().join("MiXeDCase.JPG");
        std::fs::write(&actual, b"case probe").unwrap();
        let alias = workspace.path().join("mixedcase.jpg");

        let receipt = move_to_trash(&alias).unwrap();
        assert_eq!(
            receipt.original_path().file_name().unwrap(),
            std::ffi::OsStr::new("MiXeDCase.JPG")
        );
        restore_from_trash(&receipt).unwrap();
        assert!(actual.is_file());
    }

    #[test]
    fn restore_rejects_a_path_without_a_matching_trash_receipt() {
        let ws = TempWorkspace::new("curate_missing_restore").unwrap();
        let receipt = TrashReceipt {
            original_path: ws.path().join("never-trashed.png"),
            trashed_path: None,
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

        match move_to_trash(&path) {
            Ok(receipt) => {
                assert!(!path.is_file(), "file should leave the folder after trash");
                restore_from_trash(&receipt).unwrap();
                assert!(path.is_file(), "restore should put the file back");
            }
            Err(e) => eprintln!("trash API unavailable in this environment: {e}"),
        }
    }
}
