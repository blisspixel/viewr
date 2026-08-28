//! Ephemeral workspaces for tests and explicit local verification.
//!
//! Product paths prefer in-memory data and do not sweep the shared system temp
//! root. A workspace is created atomically at one unique path and removes only
//! that path when its owning value is dropped. An existing path is never reused
//! or deleted.

use std::path::{Path, PathBuf};

/// A uniquely created directory under the process temp root.
pub struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    /// Create a uniquely named empty directory for short-lived work.
    ///
    /// # Errors
    /// Returns an I/O error if the directory cannot be created. A name collision
    /// fails without changing the existing path.
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "viewr_{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_nanos())
        ));
        Self::create(path)
    }

    fn create(path: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir(&path)?;
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
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::TempWorkspace;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn drop_removes_owned_directory() {
        let path = {
            let workspace = TempWorkspace::new("ephemeral_test").unwrap();
            let path = workspace.path().to_path_buf();
            fs::write(path.join("x.txt"), b"x").unwrap();
            assert!(path.is_dir());
            path
        };

        assert!(!path.exists(), "owned workspace must be gone after drop");
    }

    #[test]
    fn creation_collision_preserves_existing_directory() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "viewr_collision_test_{}_{}",
            std::process::id(),
            nonce
        ));
        fs::create_dir(&path).unwrap();
        let sentinel = path.join("keep.txt");
        fs::write(&sentinel, b"keep").unwrap();

        let result = TempWorkspace::create(path.clone());
        assert!(matches!(
            result,
            Err(ref error) if error.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");

        fs::remove_dir_all(path).unwrap();
    }
}
