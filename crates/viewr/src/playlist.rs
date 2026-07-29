//! Playlist management and scanning logic.

use std::path::PathBuf;

pub(crate) struct Playlist {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) index: usize,
}

pub(crate) enum ScanPurpose {
    SelectedFile(PathBuf),
    OpenFolder,
}
