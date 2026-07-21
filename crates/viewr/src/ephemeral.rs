//! Ephemeral workspaces and temp-folder hygiene.
//!
//! viewr must not leave probe/benchmark debris in the system temp folder.
//! Prefer **in-memory** paths (doctor / default benchmark) so product use writes
//! nothing under `%TEMP%` / `/tmp` at all. When a temp dir is unavoidable (tests),
//! [`TempWorkspace`] deletes it on drop, and [`scrub_stale_viewr_temps`] clears
//! any leftover `viewr_*` names from prior crashes or older builds.
//!
//! User photo libraries are never used for probes.

use std::path::{Path, PathBuf};

/// A directory under the process temp root, deleted when this value is dropped.
pub struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    /// Create a uniquely named empty directory for short-lived work.
    ///
    /// # Errors
    /// Returns an I/O error if the directory cannot be created.
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "viewr_{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    /// Borrow the workspace path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        // Best-effort: never leave probes behind, even after panics in tests.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Whether a file or directory name under the system temp root belongs to viewr.
///
/// Only matches our own naming (`viewr_…` / legacy smoke files). Never touches
/// user photo libraries or unrelated temp content.
#[must_use]
pub fn is_viewr_temp_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("viewr_")
        || lower.starts_with("viewr.")
        || lower == "viewr"
        || lower.starts_with("viewr-")
}

/// Remove leftover `viewr_*` files and directories from the system temp folder.
///
/// Safe to call at process start: only names matching [`is_viewr_temp_name`] are
/// considered. Returns how many entries were removed (best-effort; failures are
/// ignored so a locked file never aborts launch).
#[must_use]
pub fn scrub_stale_viewr_temps() -> usize {
    let root = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_viewr_temp_name(name) {
            continue;
        }
        let path = entry.path();
        // Never follow symlinks outside temp; only remove the entry itself.
        let ok = if path.is_dir() {
            std::fs::remove_dir_all(&path).is_ok()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if ok {
            removed = removed.saturating_add(1);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::{TempWorkspace, is_viewr_temp_name, scrub_stale_viewr_temps};
    use std::fs;

    #[test]
    fn drop_removes_directory() {
        let path = {
            let ws = TempWorkspace::new("ephemeral_test").unwrap();
            let p = ws.path().to_path_buf();
            fs::write(p.join("x.txt"), b"x").unwrap();
            assert!(p.is_dir());
            p
        };
        assert!(!path.exists(), "temp workspace must be gone after drop");
    }

    #[test]
    fn only_viewr_prefixed_names_match() {
        assert!(is_viewr_temp_name("viewr_doctor_1_2"));
        assert!(is_viewr_temp_name("viewr_avif_smoke.avif"));
        assert!(is_viewr_temp_name("viewr_bench_99"));
        assert!(!is_viewr_temp_name("photos"));
        assert!(!is_viewr_temp_name("tmp_viewr_not_ours"));
        assert!(!is_viewr_temp_name("chrome_viewr"));
    }

    #[test]
    fn scrub_removes_orphaned_viewr_entry() {
        let root = std::env::temp_dir();
        let orphan = root.join(format!(
            "viewr_scrub_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("leak.txt"), b"should not linger").unwrap();
        assert!(orphan.is_dir());

        let n = scrub_stale_viewr_temps();
        assert!(n >= 1, "scrub should remove at least the orphan we created");
        assert!(
            !orphan.exists(),
            "orphaned viewr_* dir must be gone after scrub"
        );
    }
}
