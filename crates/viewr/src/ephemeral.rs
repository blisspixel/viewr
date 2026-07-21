//! Ephemeral directories that are always removed on drop.
//!
//! viewr must not leave probe/benchmark debris in the system temp folder after
//! doctor or benchmark runs. User photo libraries are never used for this.

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
        // Best-effort: never leave doctor/benchmark probes behind.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::TempWorkspace;
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
}
