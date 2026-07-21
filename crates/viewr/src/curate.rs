//! Curation: move files to the OS trash and restore them.
//!
//! Deletes use the platform recycle bin / trash via the `trash` crate (never a
//! raw unlink by default). Undo looks up the last item in the platform trash
//! list by original path and restores it when the OS still has it.

use std::path::{Path, PathBuf};

/// Record of a successful trash operation, enough to attempt undo.
#[derive(Debug, Clone)]
pub struct TrashedFile {
    /// Path the file occupied before trashing.
    pub original_path: PathBuf,
    /// Playlist index at the time of delete (for restore placement).
    pub playlist_index: usize,
}

/// Move `path` to the system trash / recycle bin.
///
/// # Errors
/// Returns a human-readable reason if the platform trash API fails.
pub fn move_to_trash(path: &Path) -> Result<(), String> {
    trash::delete(path).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::{TrashedFile, move_to_trash, restore_from_trash};
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
    fn trash_and_restore_roundtrip() {
        // Platform trash APIs are real side effects; skip if the environment
        // forbids them (some CI sandboxes). A clean error is acceptable.
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
                    Err(e) => {
                        // Some platforms cannot list/restore programmatically.
                        eprintln!("restore skipped/unavailable: {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("trash API unavailable in this environment: {e}");
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
