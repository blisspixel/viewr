//! Filesystem helpers: recognizing image files and ordering them the way people
//! expect, so `img2` comes before `img10`. These are pure functions, fully
//! unit-tested, and never block the UI thread when used by the scanner.

use std::cmp::Ordering;
#[cfg(target_os = "windows")]
use std::ffi::{OsStr, OsString};
use std::io;
use std::io::{Seek, SeekFrom};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;

/// Result of comparing an accepted decode source with a current pathname entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageSourceMatch {
    /// The current regular entry is the same filesystem object.
    Same,
    /// The pathname now identifies a different regular object.
    Changed,
    /// No entry currently exists at the pathname.
    Missing,
    /// The current entry is a link, reparse point, directory, or other unsupported type.
    Unsupported,
    /// The operating system would not provide trustworthy identity evidence.
    Unavailable,
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume: u64,
    file_id: [u8; 16],
}

/// Live ownership of the filesystem object used to produce accepted image pixels.
///
/// The open handle prevents object-identifier reuse while the source is presented,
/// cached, or prepared for a guarded action. Identity values and paths are deliberately never exposed.
pub(crate) struct ImageSource {
    file: std::fs::File,
    identity: Option<FileIdentity>,
    markable: bool,
}

impl std::fmt::Debug for ImageSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageSource")
            .field("markable", &(self.markable && self.identity.is_some()))
            .finish_non_exhaustive()
    }
}

impl ImageSource {
    /// Open one source object for decoding and retain its identity handle.
    ///
    /// A directly selected final symlink remains viewable for compatibility, but
    /// is never markable because its displayed target and Trash entry differ.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let entry = std::fs::symlink_metadata(path)?;
        let markable = metadata_is_markable_regular(&entry);
        let file = if markable {
            open_regular_no_follow(path)?
        } else if entry.file_type().is_symlink() || entry.is_file() {
            open_file_no_atime(path).or_else(|error| {
                if error.kind() == io::ErrorKind::PermissionDenied {
                    std::fs::File::open(path)
                } else {
                    Err(error)
                }
            })?
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "image source must be a regular file",
            ));
        };
        let opened = file.metadata()?;
        if !opened.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "image source must resolve to a regular file",
            ));
        }
        if markable && !metadata_is_markable_regular(&opened) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "image source changed while it was opened",
            ));
        }
        let identity = file_identity(&file, &opened).ok();
        Ok(Self {
            file,
            identity,
            markable,
        })
    }

    /// Duplicate the accepted source handle and rewind it for a decoder.
    pub(crate) fn clone_for_decode(&self) -> io::Result<std::fs::File> {
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }

    #[cfg(test)]
    pub(crate) fn open_without_identity_for_test(path: &Path) -> io::Result<Self> {
        let mut source = Self::open(path)?;
        source.identity = None;
        Ok(source)
    }

    /// Compare this retained source with the current final pathname entry without
    /// following a link or accepting a non-regular object.
    #[must_use]
    pub(crate) fn matches_path(&self, path: &Path) -> ImageSourceMatch {
        if !self.markable {
            return ImageSourceMatch::Unsupported;
        }
        let Some(expected_identity) = self.identity else {
            return ImageSourceMatch::Unavailable;
        };
        let entry = match std::fs::symlink_metadata(path) {
            Ok(entry) => entry,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return ImageSourceMatch::Missing;
            }
            Err(_) => return ImageSourceMatch::Unavailable,
        };
        if !metadata_is_markable_regular(&entry) {
            return ImageSourceMatch::Unsupported;
        }
        let file = match open_regular_no_follow(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return ImageSourceMatch::Missing;
            }
            Err(_) => return ImageSourceMatch::Unavailable,
        };
        let opened = match file.metadata() {
            Ok(opened) if metadata_is_markable_regular(&opened) => opened,
            Ok(_) => return ImageSourceMatch::Unsupported,
            Err(_) => return ImageSourceMatch::Unavailable,
        };
        match file_identity(&file, &opened) {
            Ok(identity) if identity == expected_identity => ImageSourceMatch::Same,
            Ok(_) => ImageSourceMatch::Changed,
            Err(_) => ImageSourceMatch::Unavailable,
        }
    }
}

#[cfg(target_os = "windows")]
fn metadata_is_markable_regular(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_markable_regular(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(unix)]
fn open_regular_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let open_with = |flags| {
        let mut options = std::fs::OpenOptions::new();
        options.read(true).custom_flags(flags);
        options.open(path)
    };

    #[cfg(target_os = "linux")]
    {
        open_with(libc::O_NOFOLLOW | libc::O_NOATIME).or_else(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                open_with(libc::O_NOFOLLOW)
            } else {
                Err(error)
            }
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        open_with(libc::O_NOFOLLOW)
    }
}

#[cfg(target_os = "windows")]
fn open_regular_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

#[cfg(unix)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "shared platform identity boundary is fallible on Windows and callers handle one Result contract"
)]
fn file_identity(_file: &std::fs::File, metadata: &std::fs::Metadata) -> io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited read-only Win32 file-identity query
fn file_identity(file: &std::fs::File, _metadata: &std::fs::Metadata) -> io::Result<FileIdentity> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };

    let mut info = MaybeUninit::<FILE_ID_INFO>::uninit();
    let size = u32::try_from(size_of::<FILE_ID_INFO>())
        .map_err(|_| io::Error::other("file identity structure is too large"))?;
    // SAFETY: `file` owns a valid handle for the duration of the call. `info`
    // points to writable storage of exactly `size` bytes, and the API does not
    // retain either pointer.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful call initialized the complete FILE_ID_INFO buffer.
    let info = unsafe { info.assume_init() };
    Ok(FileIdentity {
        volume: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    })
}

/// File extensions decoded by the always-on pure-Rust core.
///
/// This is public so decode fuzzing and capability reporting consume the same
/// source of truth as folder navigation.
pub const CORE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "bmp", "tif", "tiff", "ico", "qoi", "tga", "ppm", "pgm",
    "pbm", "pnm", "hdr", "exr", "ff", "dds", "jxl", "svg",
];

/// Desktop MIME association for each always-on core extension.
///
/// Packaging checks derive their advertised MIME set from this table, keeping
/// Linux file associations tied to the decoder extension source of truth.
pub const CORE_MIME_ASSOCIATIONS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("png", "image/png"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("ico", "image/vnd.microsoft.icon"),
    ("qoi", "image/qoi"),
    ("tga", "image/x-tga"),
    ("ppm", "image/x-portable-pixmap"),
    ("pgm", "image/x-portable-graymap"),
    ("pbm", "image/x-portable-bitmap"),
    ("pnm", "image/x-portable-anymap"),
    ("hdr", "image/vnd.radiance"),
    ("exr", "image/x-exr"),
    ("ff", "image/x-farbfeld"),
    ("dds", "image/vnd.ms-dds"),
    ("jxl", "image/jxl"),
    ("svg", "image/svg+xml"),
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

/// Resolve a file path the same way the platform trash integration does.
///
/// Only the parent directory is canonicalized. The final component is kept
/// intact so a symlink itself remains the selected item rather than silently
/// changing the operation to its target.
///
/// # Errors
/// Returns an I/O error for an empty path, a filesystem root, an unavailable
/// current directory, or a parent directory that cannot be canonicalized.
pub fn canonical_file_path(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "path is empty"));
    }
    let target = if path.is_relative() {
        std::env::current_dir()?.join(path)
    } else {
        path.to_owned()
    };
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the filesystem root is not a file path",
        )
    })?;
    let canonical_parent = parent.canonicalize()?;
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    Ok(canonical_parent.join(file_name))
}

pub(crate) fn canonical_existing_file_path(path: &Path) -> io::Result<PathBuf> {
    let canonical = canonical_file_path(path)?;
    #[cfg(target_os = "windows")]
    {
        let parent = canonical.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
        })?;
        let requested = canonical
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
        Ok(parent.join(actual_windows_file_name(parent, requested)?))
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(canonical)
    }
}

#[cfg(target_os = "windows")]
fn actual_windows_file_name(parent: &Path, requested: &OsStr) -> io::Result<OsString> {
    let mut exact_match = None;
    let mut folded_match = None;
    let mut folded_ambiguous = false;
    for entry in std::fs::read_dir(parent)? {
        let name = entry?.file_name();
        if name == requested {
            exact_match = Some(name);
        } else if windows_os_str_eq_ignore_case(&name, requested) {
            if folded_match.is_some() {
                folded_ambiguous = true;
            } else {
                folded_match = Some(name);
            }
        }
    }
    if let Some(name) = exact_match {
        return Ok(name);
    }
    if folded_ambiguous {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "multiple directory entries match the path casing",
        ));
    }
    Ok(folded_match.unwrap_or_else(|| requested.to_owned()))
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited Win32 ordinal filename comparison
fn windows_os_str_eq_ignore_case(left: &OsStr, right: &OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let left = left.encode_wide().collect::<Vec<_>>();
    let right = right.encode_wide().collect::<Vec<_>>();
    let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
    else {
        return false;
    };
    // SAFETY: Both pointers reference initialized UTF-16 buffers for the exact
    // explicit lengths supplied. `CompareStringOrdinal` does not retain them.
    unsafe {
        CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1) == CSTR_EQUAL
    }
}

/// Read and naturally sort the supported regular files in `directory`.
///
/// A failure to open the directory is returned so sandboxed callers can ask
/// the user for explicit directory access. Entries that disappear or become
/// unreadable during the scan are skipped, which keeps one unrelated race from
/// invalidating an otherwise usable folder. Symlinks are excluded so automatic
/// browsing never follows an entry outside the selected directory; a directly
/// selected file retains the separate [`canonical_file_path`] behavior.
///
/// # Errors
/// Returns the filesystem error from opening `directory`.
pub fn scan_images(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let directory = directory.canonicalize()?;
    let entries = std::fs::read_dir(directory)?;
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !is_supported_image(&path) {
                return None;
            }
            let file_type = entry.file_type().ok()?;
            file_type.is_file().then_some(path)
        })
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

/// Open a file while making a best-effort attempt to avoid updating its
/// access time (`atime`). This is a privacy-first measure to prevent the OS
/// from leaving a forensic trail that the file was read.
pub fn open_file_no_atime(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOATIME flag on Linux
        options.custom_flags(libc::O_NOATIME);
    }

    options.open(path)
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::canonical_existing_file_path;
    use super::{
        CORE_EXTENSIONS, CORE_MIME_ASSOCIATIONS, canonical_file_path, is_core_format,
        is_supported_image, is_worker_format, natural_cmp, scan_images, supported_extensions,
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
    fn every_core_extension_has_exactly_one_mime_association() {
        let mapped_extensions = CORE_MIME_ASSOCIATIONS
            .iter()
            .map(|(extension, _)| *extension)
            .collect::<std::collections::HashSet<_>>();
        let core_extensions = CORE_EXTENSIONS
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();

        assert_eq!(CORE_MIME_ASSOCIATIONS.len(), mapped_extensions.len());
        assert_eq!(mapped_extensions, core_extensions);
        assert!(
            CORE_MIME_ASSOCIATIONS
                .iter()
                .all(|(_, mime_type)| mime_type.starts_with("image/"))
        );
    }

    #[test]
    fn canonical_file_path_resolves_relative_parent_without_following_final_item() {
        let canonical = canonical_file_path(Path::new("Cargo.toml")).unwrap();
        assert_eq!(
            canonical,
            std::env::current_dir()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("Cargo.toml")
        );
        assert!(canonical_file_path(Path::new("")).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn canonical_existing_file_path_captures_actual_windows_filename_case() {
        let workspace = TempWorkspace::new("canonical_file_case").unwrap();
        fs::write(workspace.path().join("MiXeDCase.JPG"), b"case probe").unwrap();
        let alias = workspace.path().join("mixedcase.jpg");
        let canonical = canonical_existing_file_path(&alias).unwrap();
        assert_eq!(
            canonical.file_name().unwrap(),
            std::ffi::OsStr::new("MiXeDCase.JPG")
        );
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

        let aliased_directory = workspace.path().join("nested.png").join("..");
        let paths = scan_images(&aliased_directory).unwrap();
        let canonical_directory = workspace.path().canonicalize().unwrap();
        assert!(
            paths
                .iter()
                .all(|path| path.parent() == Some(canonical_directory.as_path()))
        );
    }

    #[test]
    fn scan_reports_an_unopenable_directory() {
        let workspace = TempWorkspace::new("folder_scan_file").unwrap();
        let file = workspace.path().join("image.png");
        fs::write(&file, b"fixture").unwrap();
        assert!(scan_images(&file).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn automatic_scan_excludes_file_symlinks() {
        use std::os::unix::fs::symlink;

        let workspace = TempWorkspace::new("folder_scan_symlink").unwrap();
        let outside = TempWorkspace::new("folder_scan_symlink_target").unwrap();
        let target = outside.path().join("outside.png");
        fs::write(&target, b"fixture").unwrap();
        let link = workspace.path().join("linked.png");
        symlink(&target, &link).unwrap();

        assert!(canonical_file_path(&link).unwrap().ends_with("linked.png"));
        assert!(scan_images(workspace.path()).unwrap().is_empty());
    }
}
