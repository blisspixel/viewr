//! Bounded persistence for the default folder order.
//!
//! The file contains one validated word and no path, image state, or activity
//! history. Missing state quietly selects Latest First.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use crate::fs::FolderSort;

const MAX_PREFERENCE_BYTES: u64 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Load {
    Loaded(FolderSort),
    Missing,
    Recovered(Recovery),
}

impl Load {
    pub(crate) const fn sort(self) -> FolderSort {
        match self {
            Self::Loaded(sort) => sort,
            Self::Missing | Self::Recovered(_) => FolderSort::Latest,
        }
    }

    pub(crate) const fn recovery(self) -> Option<Recovery> {
        match self {
            Self::Recovered(recovery) => Some(recovery),
            Self::Loaded(_) | Self::Missing => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recovery {
    Invalid,
    Oversized,
    Unreadable,
    ConfigurationUnavailable,
}

impl Recovery {
    pub(crate) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Oversized => "oversized",
            Self::Unreadable => "unreadable",
            Self::ConfigurationUnavailable => "configuration-unavailable",
        }
    }

    pub(crate) const fn notice(self) -> &'static str {
        match self {
            Self::Invalid | Self::Oversized | Self::Unreadable | Self::ConfigurationUnavailable => {
                "Could not restore saved folder sort. Using Latest First."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveError {
    ConfigurationUnavailable,
    DirectoryUnavailable,
    TemporaryFileUnavailable,
    WriteFailed,
    SyncFailed,
    ReplaceFailed,
}

impl SaveError {
    pub(crate) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::ConfigurationUnavailable => "configuration-unavailable",
            Self::DirectoryUnavailable => "directory-unavailable",
            Self::TemporaryFileUnavailable => "temporary-file-unavailable",
            Self::WriteFailed => "write-failed",
            Self::SyncFailed => "sync-failed",
            Self::ReplaceFailed => "replace-failed",
        }
    }
}

pub(crate) const fn save_failure_message() -> &'static str {
    "Folder sort changed for this session but could not be remembered. Check local configuration storage, then choose it again."
}

pub(crate) fn load() -> Load {
    load_from_path(preference_path().as_deref())
}

pub(crate) fn save(sort: FolderSort) -> Result<(), SaveError> {
    let path = preference_path().ok_or(SaveError::ConfigurationUnavailable)?;
    save_to(&path, sort)
}

fn load_from_path(path: Option<&Path>) -> Load {
    path.map_or(
        Load::Recovered(Recovery::ConfigurationUnavailable),
        load_from,
    )
}

fn load_from(path: &Path) -> Load {
    let file = match crate::fs::open_file_no_atime(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Load::Missing,
        Err(_) => return Load::Recovered(Recovery::Unreadable),
    };
    let Ok(metadata) = file.metadata() else {
        return Load::Recovered(Recovery::Unreadable);
    };
    if !metadata.is_file() {
        return Load::Recovered(Recovery::Unreadable);
    }
    let length = metadata.len();
    if length > MAX_PREFERENCE_BYTES {
        return Load::Recovered(Recovery::Oversized);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    if file
        .take(MAX_PREFERENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Load::Recovered(Recovery::Unreadable);
    }
    if bytes.len() as u64 > MAX_PREFERENCE_BYTES {
        return Load::Recovered(Recovery::Oversized);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Load::Recovered(Recovery::Invalid);
    };
    FolderSort::parse(value).map_or(Load::Recovered(Recovery::Invalid), Load::Loaded)
}

fn save_to(path: &Path, sort: FolderSort) -> Result<(), SaveError> {
    let parent = path.parent().ok_or(SaveError::ConfigurationUnavailable)?;
    fs::create_dir_all(parent).map_err(|_| SaveError::DirectoryUnavailable)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| SaveError::TemporaryFileUnavailable)?;
    temporary
        .write_all(format!("{}\n", sort.as_str()).as_bytes())
        .map_err(|_| SaveError::WriteFailed)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| SaveError::SyncFailed)?;
    temporary
        .persist(path)
        .map_err(|_| SaveError::ReplaceFailed)?;
    Ok(())
}

fn preference_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("viewr").join("folder-sort"));
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("viewr")
                    .join("folder-sort")
            });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Some(path.join("viewr").join("folder-sort"));
        }
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("viewr").join("folder-sort"));
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::{
        Load, Recovery, SaveError, load_from, load_from_path, save_failure_message, save_to,
    };
    use crate::fs::FolderSort;

    #[test]
    fn preference_round_trips_and_classifies_unusable_state() {
        let workspace = crate::ephemeral::TempWorkspace::new("folder_sort_preference").unwrap();
        let path = workspace.path().join("nested").join("folder-sort");
        assert_eq!(load_from(&path), Load::Missing);
        assert_eq!(Load::Missing.sort(), FolderSort::Latest);
        assert_eq!(Load::Missing.recovery(), None);
        assert_eq!(
            load_from_path(None),
            Load::Recovered(Recovery::ConfigurationUnavailable)
        );
        for sort in [FolderSort::Latest, FolderSort::Name] {
            save_to(&path, sort).unwrap();
            assert_eq!(load_from(&path), Load::Loaded(sort));
            let entries = std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(entries.len(), 1, "persistence left a temporary file");
            assert_eq!(entries[0].path(), path);
        }
        std::fs::write(&path, "not-a-sort\n").unwrap();
        assert_eq!(load_from(&path), Load::Recovered(Recovery::Invalid));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not-a-sort\n");
        std::fs::write(&path, [0xFF, 0xFE]).unwrap();
        assert_eq!(load_from(&path), Load::Recovered(Recovery::Invalid));
        std::fs::write(&path, "x".repeat(64)).unwrap();
        assert_eq!(load_from(&path), Load::Recovered(Recovery::Oversized));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(load_from(&path), Load::Recovered(Recovery::Unreadable));
    }

    #[test]
    fn recovery_and_save_failures_are_fixed_and_path_private() {
        for (recovery, category) in [
            (Recovery::Invalid, "invalid"),
            (Recovery::Oversized, "oversized"),
            (Recovery::Unreadable, "unreadable"),
            (
                Recovery::ConfigurationUnavailable,
                "configuration-unavailable",
            ),
        ] {
            assert_eq!(recovery.diagnostic_name(), category);
            assert_eq!(
                recovery.notice(),
                "Could not restore saved folder sort. Using Latest First."
            );
        }
        for (failure, category) in [
            (
                SaveError::ConfigurationUnavailable,
                "configuration-unavailable",
            ),
            (SaveError::DirectoryUnavailable, "directory-unavailable"),
            (
                SaveError::TemporaryFileUnavailable,
                "temporary-file-unavailable",
            ),
            (SaveError::WriteFailed, "write-failed"),
            (SaveError::SyncFailed, "sync-failed"),
            (SaveError::ReplaceFailed, "replace-failed"),
        ] {
            assert_eq!(failure.diagnostic_name(), category);
        }
        let message = save_failure_message();
        for private_fragment in ["C:\\Users\\private", "/home/private", "access denied"] {
            assert!(!message.contains(private_fragment));
        }
    }
}
