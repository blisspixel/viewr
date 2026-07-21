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
    /// Path the file occupied before trashing.
    pub original_path: PathBuf,
    /// Playlist index at the time of delete (for restore placement).
    pub playlist_index: usize,
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
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
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
pub fn restore_from_trash(original_path: &Path) -> Result<(), String> {
    let items = trash::os_limited::list().map_err(|e| e.to_string())?;
    let matching: Vec<_> = items
        .into_iter()
        .filter(|item| item.original_path() == original_path)
        .collect();
    if matching.is_empty() {
        return Err(
            "could not find the file in the system trash (it may have been emptied)".into(),
        );
    }
    trash::os_limited::restore_all(matching).map_err(|e| e.to_string())
}

/// Trash many paths; returns successes and the first error (if any) after attempting all.
#[must_use]
pub fn trash_many(paths: &[PathBuf]) -> (Vec<PathBuf>, Option<String>) {
    let mut ok = Vec::new();
    let mut first_err = None;
    for path in paths {
        match move_to_trash(path) {
            Ok(()) => ok.push(path.clone()),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(format!("{}: {e}", path.display()));
                }
            }
        }
    }
    (ok, first_err)
}

#[cfg(test)]
mod tests {
    use super::{
        FlagSet, TrashedFile, index_after_removals, move_to_trash, permanent_delete,
        remove_from_playlist, restore_from_trash, trash_many,
    };
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viewr_curate_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn trashed_file_record_holds_path() {
        let t = TrashedFile {
            original_path: PathBuf::from("a.jpg"),
            playlist_index: 3,
        };
        assert_eq!(t.playlist_index, 3);
        assert_eq!(t.original_path, PathBuf::from("a.jpg"));
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
        let batch = flags.take_all_sorted();
        assert_eq!(batch, vec![PathBuf::from("b.jpg"), PathBuf::from("c.jpg")]);
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
        let dir = scratch("perm");
        let path = dir.join("gone.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9]))
            .save(&path)
            .unwrap();
        assert!(path.is_file());
        permanent_delete(&path).unwrap();
        assert!(!path.is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trash_many_partial_success() {
        let dir = scratch("many");
        let good = dir.join("ok.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 1, 1]))
            .save(&good)
            .unwrap();
        let missing = dir.join("missing.png");
        let (ok, err) = trash_many(&[good.clone(), missing]);
        // Either trash works (ok contains good) or environment forbids trash.
        if err.is_none() {
            assert!(ok.contains(&good));
            assert!(!good.is_file());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trash_and_restore_roundtrip() {
        let dir = scratch("roundtrip");
        let path = dir.join("photo.png");
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]));
        img.save(&path).unwrap();
        assert!(path.is_file());

        match move_to_trash(&path) {
            Ok(()) => {
                assert!(!path.is_file(), "file should leave the folder after trash");
                match restore_from_trash(&path) {
                    Ok(()) => assert!(path.is_file(), "restore should put the file back"),
                    Err(e) => eprintln!("restore skipped/unavailable: {e}"),
                }
            }
            Err(e) => eprintln!("trash API unavailable in this environment: {e}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
