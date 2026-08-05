//! Pure generation and path currency for background work.
//!
//! The event loop owns session counters, job contexts, and side effects.
//! This module owns only whether a completed job still applies to the current
//! selection, presentation, or loaded image.

use std::path::Path;

/// True when the job targets the current selection generation and path.
#[must_use]
pub(crate) fn selected_work_is_current(
    job_generation: u64,
    job_path: &Path,
    current_generation: u64,
    selected_path: Option<&Path>,
) -> bool {
    job_generation == current_generation && selected_path == Some(job_path)
}

/// True when the job targets the current selection and the same path is still
/// the presented image at the same generation.
#[must_use]
pub(crate) fn presented_work_is_current(
    job_generation: u64,
    job_path: &Path,
    current_generation: u64,
    selected_path: Option<&Path>,
    presented_path: Option<&Path>,
) -> bool {
    selected_work_is_current(job_generation, job_path, current_generation, selected_path)
        && presented_path == Some(job_path)
}

/// True when the job targets the currently loaded path at the current generation.
///
/// Production call sites are Windows Open With verification today; the pure
/// contract stays available to unit tests on every platform.
#[cfg(any(test, target_os = "windows"))]
#[must_use]
pub(crate) fn loaded_work_is_current(
    job_generation: u64,
    job_path: &Path,
    current_generation: u64,
    loaded_path: Option<&Path>,
) -> bool {
    job_generation == current_generation && loaded_path == Some(job_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn selected_work_requires_exact_generation_and_selected_path() {
        let path = Path::new("album/large.png");
        assert!(selected_work_is_current(17, path, 17, Some(path)));
        assert!(!selected_work_is_current(16, path, 17, Some(path)));
        assert!(!selected_work_is_current(
            17,
            path,
            17,
            Some(Path::new("album/other.png"))
        ));
        assert!(!selected_work_is_current(17, path, 17, None));
    }

    #[test]
    fn presented_work_requires_selected_and_presented_path_agreement() {
        let path = PathBuf::from("current.jpg");
        let other = Path::new("other.jpg");
        assert!(presented_work_is_current(
            8,
            &path,
            8,
            Some(&path),
            Some(&path)
        ));
        assert!(!presented_work_is_current(
            7,
            &path,
            8,
            Some(&path),
            Some(&path)
        ));
        assert!(!presented_work_is_current(
            8,
            &path,
            8,
            Some(other),
            Some(&path)
        ));
        assert!(!presented_work_is_current(
            8,
            &path,
            8,
            Some(&path),
            Some(other)
        ));
        assert!(!presented_work_is_current(8, &path, 8, Some(&path), None));
    }

    #[test]
    fn loaded_work_requires_exact_generation_and_loaded_path() {
        let path = Path::new("open-with.jpg");
        assert!(loaded_work_is_current(3, path, 3, Some(path)));
        assert!(!loaded_work_is_current(2, path, 3, Some(path)));
        assert!(!loaded_work_is_current(
            3,
            path,
            3,
            Some(Path::new("other.jpg"))
        ));
        assert!(!loaded_work_is_current(3, path, 3, None));
    }
}
