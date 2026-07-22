//! Filesystem helpers: recognizing image files and ordering them the way people
//! expect, so `img2` comes before `img10`. These are pure functions, fully
//! unit-tested, and never block the UI thread when used by the scanner.

use std::cmp::Ordering;
use std::io;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;

/// File extensions decoded by the always-on pure-Rust core.
///
/// This is public so decode fuzzing and capability reporting consume the same
/// source of truth as folder navigation.
pub const CORE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "ico", "qoi", "tga", "ppm", "pgm",
    "pbm", "pnm", "hdr", "exr", "ff", "dds", "jxl", "svg",
];

/// Formats decoded only by the `viewr-decode` worker (C-backed or deferred).
/// Listed so folder navigation includes them; actual success depends on a
/// co-located worker binary and its Cargo features (see `docs/FORMATS.md`).
const WORKER_EXTENSIONS: &[&str] = &[
    "avif", "heic", "heif", "cr2", "nef", "arw", "dng", "rw2", "orf", "raf",
];

/// Iterate over every recognized image extension without maintaining a second
/// dialog-specific list.
pub fn supported_extensions() -> impl Iterator<Item = &'static str> {
    CORE_EXTENSIONS.iter().chain(WORKER_EXTENSIONS).copied()
}

/// Return `true` if `path` has an extension viewr knows about (core or worker).
#[must_use]
pub fn is_supported_image(path: &Path) -> bool {
    extension_kind(path).is_some()
}

/// Return `true` if the file must be decoded in the isolated worker process.
#[must_use]
pub fn is_worker_format(path: &Path) -> bool {
    matches!(extension_kind(path), Some(ExtensionKind::Worker))
}

/// Return `true` if the file is a core pure-Rust format.
#[must_use]
pub fn is_core_format(path: &Path) -> bool {
    matches!(extension_kind(path), Some(ExtensionKind::Core))
}

/// Read and naturally sort the supported regular files in `directory`.
///
/// A failure to open the directory is returned so sandboxed callers can ask
/// the user for explicit directory access. Entries that disappear or become
/// unreadable during the scan are skipped, which keeps one unrelated race from
/// invalidating an otherwise usable folder.
///
/// # Errors
/// Returns the filesystem error from opening `directory`.
pub fn scan_images(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(directory)?;
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_supported_image(path))
        .collect::<Vec<_>>();
    files.sort_by(|a, b| {
        let a_name = a.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let b_name = b.file_name().and_then(|name| name.to_str()).unwrap_or("");
        natural_cmp(a_name, b_name)
    });
    Ok(files)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionKind {
    Core,
    Worker,
}

fn extension_kind(path: &Path) -> Option<ExtensionKind> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)?;
    if CORE_EXTENSIONS.contains(&ext.as_str()) {
        Some(ExtensionKind::Core)
    } else if WORKER_EXTENSIONS.contains(&ext.as_str()) {
        Some(ExtensionKind::Worker)
    } else {
        None
    }
}

/// Compare two file names the way a human reads them: runs of digits are
/// compared by numeric value, so `img2` orders before `img10`. Comparison is
/// case-insensitive elsewhere.
#[must_use]
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    match take_number(&mut ai).cmp(&take_number(&mut bi)) {
                        Ordering::Equal => {}
                        non_eq => return non_eq,
                    }
                } else {
                    match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        non_eq => return non_eq,
                    }
                }
            }
        }
    }
}

/// Consume a run of ASCII digits from `it` and return its numeric value,
/// saturating rather than overflowing on absurdly long runs.
fn take_number(it: &mut Peekable<Chars>) -> u64 {
    let mut n: u64 = 0;
    while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(u64::from(d));
        it.next();
    }
    n
}

#[cfg(test)]
mod tests {
    use super::{
        is_core_format, is_supported_image, is_worker_format, natural_cmp, scan_images,
        supported_extensions,
    };
    use crate::ephemeral::TempWorkspace;
    use std::cmp::Ordering;
    use std::fs;
    use std::path::Path;

    #[test]
    fn recognizes_common_images() {
        assert!(is_supported_image(Path::new("a.jpg")));
        assert!(is_supported_image(Path::new("A.PNG")));
        assert!(is_supported_image(Path::new("x.jxl")));
        assert!(is_supported_image(Path::new("/some/dir/photo.webp")));
        assert!(is_core_format(Path::new("a.svg")));
    }

    #[test]
    fn recognizes_worker_formats_for_browsing() {
        assert!(is_supported_image(Path::new("shot.avif")));
        assert!(is_supported_image(Path::new("phone.HEIC")));
        assert!(is_supported_image(Path::new("raw.CR2")));
        assert!(is_worker_format(Path::new("shot.avif")));
        assert!(!is_core_format(Path::new("shot.avif")));
        assert!(!is_worker_format(Path::new("a.png")));
    }

    #[test]
    fn rejects_non_images() {
        assert!(!is_supported_image(Path::new("notes.txt")));
        assert!(!is_supported_image(Path::new("noext")));
        assert!(!is_supported_image(Path::new("archive.zip")));
    }

    #[test]
    fn natural_order_numbers() {
        assert_eq!(natural_cmp("img2.jpg", "img10.jpg"), Ordering::Less);
        assert_eq!(natural_cmp("img10.jpg", "img2.jpg"), Ordering::Greater);
        assert_eq!(natural_cmp("a.jpg", "a.jpg"), Ordering::Equal);
    }

    #[test]
    fn natural_order_case_insensitive() {
        assert_eq!(natural_cmp("Photo.jpg", "photo.jpg"), Ordering::Equal);
    }

    #[test]
    fn natural_sort_orders_a_list() {
        let mut v = vec!["img10", "img2", "img1", "img100"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["img1", "img2", "img10", "img100"]);
    }

    #[test]
    fn extension_iterator_is_complete_and_unique() {
        let extensions = supported_extensions().collect::<Vec<_>>();
        let unique = extensions
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(extensions.len(), unique.len());
        assert!(extensions.contains(&"png"));
        assert!(extensions.contains(&"avif"));
    }

    #[test]
    fn scans_only_supported_files_in_natural_order() {
        let workspace = TempWorkspace::new("folder_scan").unwrap();
        for name in ["img10.png", "img2.jpg", "notes.txt", "img1.avif"] {
            fs::write(workspace.path().join(name), b"fixture").unwrap();
        }
        fs::create_dir(workspace.path().join("nested.png")).unwrap();

        let names = scan_images(workspace.path())
            .unwrap()
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, ["img1.avif", "img2.jpg", "img10.png"]);
    }

    #[test]
    fn scan_reports_an_unopenable_directory() {
        let workspace = TempWorkspace::new("folder_scan_file").unwrap();
        let file = workspace.path().join("image.png");
        fs::write(&file, b"fixture").unwrap();
        assert!(scan_images(&file).is_err());
    }
}
