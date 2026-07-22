//! Ephemeral workspaces and temp-folder hygiene.
//!
//! viewr must not leave probe/benchmark debris in the system temp folder.
//! Prefer **in-memory** paths (doctor / default benchmark) so product use writes
//! nothing under `%TEMP%` / `/tmp` at all. When a temp dir is unavoidable (tests),
//! [`TempWorkspace`] deletes it on drop, and [`scrub_stale_viewr_temps`] clears
//! leftovers from prior crashes or older builds **without** touching live
//! workspaces of the current process.
//!
//! User photo libraries are never used for probes.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

const WORKSPACE_LOCK: &str = ".viewr-lock";

/// A directory under the process temp root, deleted when this value is dropped.
pub struct TempWorkspace {
    path: PathBuf,
    lock: Option<File>,
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
        let lock = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path.join(WORKSPACE_LOCK))
        {
            Ok(lock) => lock,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&path);
                return Err(error);
            }
        };
        if let Err(error) = lock.lock() {
            drop(lock);
            let _ = std::fs::remove_dir_all(&path);
            return Err(error);
        }
        Ok(Self {
            path,
            lock: Some(lock),
        })
    }

    /// Borrow the workspace path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _ = lock.unlock();
            drop(lock);
        }
        // Best-effort: never leave probes behind, even after panics in tests.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn workspace_is_locked(path: &Path) -> bool {
    let lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.join(WORKSPACE_LOCK))
    {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    match lock.try_lock() {
        Ok(()) => {
            let _ = lock.unlock();
            false
        }
        Err(_) => true,
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

/// Extract the process id embedded in a `TempWorkspace` name, if present.
///
/// Only the current form is recognized: `viewr_{prefix}_{pid}_{nanos}` where
/// **both** trailing components are decimal integers. A single trailing number
/// (e.g. `viewr_avif_smoke_orphan_42`) is **not** treated as a live PID — those
/// are always safe to scrub.
fn embedded_pid(name: &str) -> Option<u32> {
    // Strip optional extension (e.g. viewr_job_smoke.avif).
    let stem = name.split('.').next().unwrap_or(name);
    let mut parts = stem.split('_');
    if parts.next()? != "viewr" {
        return None;
    }
    let rest: Vec<&str> = parts.collect();
    if rest.len() < 2 {
        return None;
    }
    let nanos = rest[rest.len() - 1];
    let pid = rest[rest.len() - 2];
    if nanos.chars().all(|c| c.is_ascii_digit()) && pid.chars().all(|c| c.is_ascii_digit()) {
        return pid.parse().ok();
    }
    None
}

/// True when this entry is safe to delete (not a live workspace of *this* process).
///
/// Entries tagged with another PID are treated as leftovers from a previous run
/// (product paths no longer use temp dirs; tests always use the current PID).
fn is_safe_to_scrub(name: &str) -> bool {
    if !is_viewr_temp_name(name) {
        return false;
    }
    match embedded_pid(name) {
        Some(pid) if pid == std::process::id() => false,
        Some(_) | None => true,
    }
}

/// Remove leftover `viewr_*` files and directories from the system temp folder.
///
/// Skips workspaces belonging to the current process and any workspace holding
/// its lock, so concurrent processes are not deleted mid-run. Returns how many
/// entries were removed (best-effort).
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
        if !is_safe_to_scrub(name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() && workspace_is_locked(&path) {
            continue;
        }
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
    use super::{
        TempWorkspace, embedded_pid, is_safe_to_scrub, is_viewr_temp_name, scrub_stale_viewr_temps,
    };
    use std::fs::{self, OpenOptions};
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SCRUB_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial_scrub_test() -> MutexGuard<'static, ()> {
        SCRUB_TEST_LOCK
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

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
    fn embedded_pid_parses_workspace_names() {
        assert_eq!(embedded_pid("viewr_edit_exif_4242_999"), Some(4242));
        // Single trailing number is not a live workspace id.
        assert_eq!(embedded_pid("viewr_bench_77"), None);
        assert_eq!(embedded_pid("viewr_avif_smoke.avif"), None);
        assert_eq!(embedded_pid("viewr_avif_smoke_orphan_32212.avif"), None);
    }

    #[test]
    fn scrub_skips_current_process_workspaces() {
        let _guard = serial_scrub_test();
        let ws = TempWorkspace::new("live_scrub_guard").unwrap();
        let name = ws
            .path()
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_string();
        assert!(
            !is_safe_to_scrub(&name),
            "must not scrub live workspace of this process"
        );
        let _ = scrub_stale_viewr_temps();
        assert!(
            ws.path().is_dir(),
            "scrub must leave current-process TempWorkspace alone"
        );
    }

    #[test]
    fn scrub_skips_locked_workspace_from_another_process() {
        let _guard = serial_scrub_test();
        let other_pid = std::process::id().checked_add(1).unwrap_or(1);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("viewr_other_live_{other_pid}_{nonce}"));
        fs::create_dir(&path).unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path.join(".viewr-lock"))
            .unwrap();
        lock.lock().unwrap();

        let _ = scrub_stale_viewr_temps();
        assert!(
            path.is_dir(),
            "scrub must leave a locked workspace from another process alone"
        );

        lock.unlock().unwrap();
        drop(lock);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn scrub_removes_legacy_orphans_without_pid() {
        let _guard = serial_scrub_test();
        let root = std::env::temp_dir();
        // No numeric tail → no embedded PID → always safe to scrub.
        let orphan = root.join(format!(
            "viewr_avif_smoke_orphan_{}.avif",
            std::process::id()
        ));
        // Ensure the name is classified as scrub-safe even under concurrent tests.
        let name = orphan.file_name().and_then(|s| s.to_str()).unwrap();
        assert!(
            is_safe_to_scrub(name),
            "legacy smoke name must be scrub-safe: {name}"
        );
        fs::write(&orphan, b"not an image").unwrap();
        assert!(orphan.is_file());
        let _ = scrub_stale_viewr_temps();
        assert!(
            !orphan.exists(),
            "legacy viewr_* smoke file must be gone after scrub"
        );
    }
}
