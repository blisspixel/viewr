//! Filesystem helpers: recognizing image files and ordering them the way people
//! expect, so `img2` comes before `img10`. These are pure functions, fully
//! unit-tested, and never block the UI thread when used by the scanner.

use std::cmp::Ordering;
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::ffi::OsString;
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;
use std::sync::{Mutex, MutexGuard};

pub(crate) const MAX_FOLDER_IMAGES: usize = 100_000;
pub(crate) const MAX_FOLDER_PATH_BYTES: usize = 64 * 1024 * 1024;
const MAX_ACCEPTED_SOURCE_BYTES: u64 = viewr_protocol::MAX_ENCODED_INPUT_BYTES;

#[derive(Debug)]
pub(crate) enum ScanImagesError {
    Io(io::Error),
    Cancelled,
    LimitExceeded,
    PathBudgetExceeded,
    WorkerStopped,
}

impl std::fmt::Display for ScanImagesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Cancelled => formatter.write_str("folder scan was superseded"),
            Self::LimitExceeded => formatter.write_str("folder image limit exceeded"),
            Self::PathBudgetExceeded => {
                formatter.write_str("folder path-data safety limit exceeded")
            }
            Self::WorkerStopped => formatter.write_str("folder scan worker stopped unexpectedly"),
        }
    }
}

impl std::error::Error for ScanImagesError {}

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

/// Opaque identity evidence captured while a regular folder entry is scanned.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScanProvenance {
    identity: FileIdentity,
    version: FileVersion,
}

impl ScanProvenance {
    /// Whether two scan records name the same filesystem object.
    ///
    /// Rename updates version evidence while preserving object identity, so a
    /// folder refresh can follow the object to its new pathname.
    #[must_use]
    pub(crate) fn same_object(self, other: Self) -> bool {
        self.identity == other.identity
    }
}

/// Identity plus version for the folder currently being browsed.
///
/// Adding, renaming, or removing a child changes directory version evidence
/// without replacing the directory object. The session watcher uses that
/// change to rescan membership.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectoryStamp {
    identity: FileIdentity,
    version: FileVersion,
    child_count: u32,
}

/// Snapshot the ordinary directory at `path` without following its final component.
#[must_use]
pub(crate) fn directory_stamp(path: &Path) -> Option<DirectoryStamp> {
    let source = DirectorySource::open(path).ok()?;
    let metadata = source.file.metadata().ok()?;
    metadata_is_plain_directory(&metadata).then_some(())?;
    let mut child_count = 0_u32;
    for entry in std::fs::read_dir(path).ok()? {
        // A single unreadable child must not make the whole stamp unavailable.
        // Count the attempt so a disappearing entry still changes membership.
        let _ = entry;
        child_count = child_count.saturating_add(1);
    }
    Some(DirectoryStamp {
        identity: source.identity,
        version: file_version(&source.file, &metadata).ok()?,
        child_count,
    })
}

/// One automatically discovered image and the object identity observed at scan time.
#[derive(Clone)]
pub(crate) struct ScannedImage {
    path: PathBuf,
    provenance: ScanProvenance,
}

impl ScannedImage {
    #[must_use]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub(crate) const fn provenance(&self) -> ScanProvenance {
        self.provenance
    }

    #[must_use]
    pub(crate) fn into_parts(self) -> (PathBuf, ScanProvenance) {
        (self.path, self.provenance)
    }
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileVersion {
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileVersion {
    length: u64,
    last_write_time: i64,
    change_time: i64,
}

#[cfg(target_os = "windows")]
const SHA256_BYTES: usize = 32;
#[cfg(target_os = "windows")]
pub(crate) const CONTENT_WITNESS_CHUNK_BYTES: usize = 64 * 1024;

#[cfg(target_os = "windows")]
struct Sha256Algorithm(windows_sys::Win32::Security::Cryptography::BCRYPT_ALG_HANDLE);

#[cfg(target_os = "windows")]
impl Sha256Algorithm {
    #[allow(unsafe_code)] // opens one SHA-256 provider through Windows CNG
    fn open() -> io::Result<Self> {
        use windows_sys::Win32::Security::Cryptography::{
            BCRYPT_SHA256_ALGORITHM, BCryptOpenAlgorithmProvider,
        };

        let mut algorithm = std::ptr::null_mut();
        // SAFETY: `algorithm` is writable handle storage, the SHA-256 identifier
        // is process-lifetime data, and no provider string is supplied.
        let status = unsafe {
            BCryptOpenAlgorithmProvider(
                &raw mut algorithm,
                BCRYPT_SHA256_ALGORITHM,
                std::ptr::null(),
                0,
            )
        };
        if status < 0 {
            return Err(cng_status_error(status));
        }
        if algorithm.is_null() {
            return Err(io::Error::other(
                "SHA-256 provider returned no algorithm handle",
            ));
        }
        Ok(Self(algorithm))
    }

    #[allow(unsafe_code)] // reads one fixed-size property from a live CNG provider
    fn property_u32(&self, name: windows_sys::core::PCWSTR) -> io::Result<u32> {
        use std::mem::size_of;
        use windows_sys::Win32::Security::Cryptography::BCryptGetProperty;

        let expected = u32::try_from(size_of::<u32>()).unwrap_or(u32::MAX);
        let mut output = 0_u32;
        let mut returned = 0_u32;
        // SAFETY: The provider handle is live, `output` has exactly the stated
        // capacity, and CNG does not retain any supplied pointer.
        let status = unsafe {
            BCryptGetProperty(
                self.0,
                name,
                std::ptr::from_mut(&mut output).cast(),
                expected,
                &raw mut returned,
                0,
            )
        };
        if status < 0 {
            return Err(cng_status_error(status));
        }
        if returned != expected {
            return Err(io::Error::other(
                "SHA-256 provider returned an invalid property size",
            ));
        }
        Ok(output)
    }
}

#[cfg(target_os = "windows")]
impl Drop for Sha256Algorithm {
    #[allow(unsafe_code)] // releases one exclusively owned Windows CNG provider handle
    fn drop(&mut self) {
        // SAFETY: This wrapper exclusively owns the successful provider handle.
        unsafe {
            windows_sys::Win32::Security::Cryptography::BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}

#[cfg(target_os = "windows")]
struct Sha256Hash {
    handle: windows_sys::Win32::Security::Cryptography::BCRYPT_HASH_HANDLE,
    _object: Vec<u8>,
}

#[cfg(target_os = "windows")]
impl Sha256Hash {
    #[allow(unsafe_code)] // creates one SHA-256 state object through Windows CNG
    fn new(algorithm: &Sha256Algorithm) -> io::Result<Self> {
        use windows_sys::Win32::Security::Cryptography::{
            BCRYPT_HASH_LENGTH, BCRYPT_OBJECT_LENGTH, BCryptCreateHash,
        };

        let digest_bytes = algorithm.property_u32(BCRYPT_HASH_LENGTH)?;
        if usize::try_from(digest_bytes).ok() != Some(SHA256_BYTES) {
            return Err(io::Error::other(
                "SHA-256 provider returned an invalid digest size",
            ));
        }
        let object_bytes = usize::try_from(algorithm.property_u32(BCRYPT_OBJECT_LENGTH)?)
            .ok()
            .filter(|bytes| (1..=1024 * 1024).contains(bytes))
            .ok_or_else(|| io::Error::other("SHA-256 provider returned an invalid object size"))?;
        let mut object = Vec::new();
        object
            .try_reserve_exact(object_bytes)
            .map_err(|_| io::Error::other("SHA-256 object allocation failed"))?;
        object.resize(object_bytes, 0);

        let mut hash = std::ptr::null_mut();
        // SAFETY: The provider is live, `hash` is writable handle storage, and
        // the object buffer remains stable and owned by the resulting wrapper.
        let status = unsafe {
            BCryptCreateHash(
                algorithm.0,
                &raw mut hash,
                object.as_mut_ptr(),
                u32::try_from(object.len()).unwrap_or(u32::MAX),
                std::ptr::null(),
                0,
                0,
            )
        };
        if status < 0 {
            return Err(cng_status_error(status));
        }
        if hash.is_null() {
            return Err(io::Error::other("SHA-256 provider returned no hash handle"));
        }
        Ok(Self {
            handle: hash,
            _object: object,
        })
    }

    #[allow(unsafe_code)] // submits one initialized byte slice to a live CNG hash
    fn update(&self, bytes: &[u8]) -> io::Result<()> {
        use windows_sys::Win32::Security::Cryptography::BCryptHashData;

        let length = u32::try_from(bytes.len())
            .map_err(|_| io::Error::other("SHA-256 input chunk is too large"))?;
        // SAFETY: The hash handle is live and `bytes` remains initialized for
        // the duration of this synchronous call.
        let status = unsafe { BCryptHashData(self.handle, bytes.as_ptr(), length, 0) };
        if status < 0 {
            Err(cng_status_error(status))
        } else {
            Ok(())
        }
    }

    #[allow(unsafe_code)] // writes one fixed-size SHA-256 result from a live CNG hash
    fn finish(&self) -> io::Result<[u8; SHA256_BYTES]> {
        use windows_sys::Win32::Security::Cryptography::BCryptFinishHash;

        let mut digest = [0_u8; SHA256_BYTES];
        // SAFETY: The hash handle is live and `digest` is writable storage of
        // the exact provider-reported SHA-256 digest length.
        let status = unsafe {
            BCryptFinishHash(
                self.handle,
                digest.as_mut_ptr(),
                u32::try_from(digest.len()).unwrap_or(u32::MAX),
                0,
            )
        };
        if status < 0 {
            Err(cng_status_error(status))
        } else {
            Ok(digest)
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for Sha256Hash {
    #[allow(unsafe_code)] // releases one exclusively owned Windows CNG hash handle
    fn drop(&mut self) {
        // SAFETY: This wrapper exclusively owns the successful hash handle, and
        // `_object` remains allocated until after this destructor returns.
        unsafe {
            windows_sys::Win32::Security::Cryptography::BCryptDestroyHash(self.handle);
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // converts one CNG NTSTATUS into the corresponding OS error
fn cng_status_error(status: i32) -> io::Error {
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;

    // SAFETY: `status` came from a CNG call; conversion retains no pointers.
    let code = unsafe { RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(i32::try_from(code).unwrap_or(i32::MAX))
}

/// Live ownership of the filesystem object used to produce accepted image pixels.
///
/// The open handle prevents object-identifier reuse while the source is presented,
/// cached, or prepared for a guarded action. Identity values and paths are deliberately never exposed.
pub(crate) struct ImageSource {
    file: std::fs::File,
    cursor: Mutex<()>,
    identity: Option<FileIdentity>,
    version: Option<FileVersion>,
    #[cfg(target_os = "windows")]
    content_digest: Option<[u8; SHA256_BYTES]>,
    accepted_path: Option<PathBuf>,
    markable: bool,
}

/// Serialized reader for one retained source handle.
///
/// `File::try_clone` duplicates a handle but shares its cursor on supported
/// platforms. Retaining this guard for the reader lifetime prevents concurrent
/// seeks from corrupting accepted-source decode, inspection, or export reads.
pub(crate) struct ImageSourceReader<'a> {
    file: std::fs::File,
    _cursor: MutexGuard<'a, ()>,
}

/// Read-only source used for cancellable folder-rating discovery.
///
/// It exposes retained bytes and native identity/version validation, but cannot
/// authorize writes, exports, or destructive actions. Those boundaries require
/// a full [`ImageSource`] content witness.
pub(crate) struct RatingScanSource(ImageSource);

/// Retained native identity for a pathname selected as a Save As destination.
///
/// Destination consent binds to the existing filesystem object, not to bytes
/// displayed by viewr. This type therefore exposes no reader and performs no
/// full-file content witness on the UI thread.
pub(crate) struct PathIdentitySource(ImageSource);

impl PathIdentitySource {
    fn from_opened_file(file: std::fs::File) -> io::Result<Self> {
        let opened = file.metadata()?;
        if !metadata_is_markable_regular(&opened) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Save As destination must be an ordinary file",
            ));
        }
        let identity = file_identity(&file, &opened)?;
        let version = file_version(&file, &opened)?;
        Ok(Self(ImageSource {
            file,
            cursor: Mutex::new(()),
            identity: Some(identity),
            version: Some(version),
            #[cfg(target_os = "windows")]
            content_digest: None,
            accepted_path: None,
            markable: true,
        }))
    }
}

impl std::ops::Deref for ImageSourceReader<'_> {
    type Target = std::fs::File;

    fn deref(&self) -> &Self::Target {
        &self.file
    }
}

impl Read for ImageSourceReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

impl Seek for ImageSourceReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

/// Retained identity of the directory selected as a Save As parent.
pub(crate) struct DirectorySource {
    file: std::fs::File,
    identity: FileIdentity,
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
        Self::open_while(path, || true)
    }

    /// Open one source while cooperatively stopping superseded background work.
    pub(crate) fn open_while(path: &Path, keep_going: impl FnMut() -> bool) -> io::Result<Self> {
        Self::open_with_witness_while(path, true, keep_going)
    }

    fn open_with_witness_while(
        path: &Path,
        capture_content_witness: bool,
        mut keep_going: impl FnMut() -> bool,
    ) -> io::Result<Self> {
        ensure_source_work_current(&mut keep_going)?;
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
        Self::from_opened_file(
            file,
            markable,
            Some(path.to_path_buf()),
            capture_content_witness,
            keep_going,
        )
    }

    /// Open a regular object without following its final path component.
    pub(crate) fn open_regular(path: &Path) -> io::Result<Self> {
        Self::open_regular_while(path, || true)
    }

    /// Open a regular object while cooperatively stopping superseded work.
    pub(crate) fn open_regular_while(
        path: &Path,
        keep_going: impl FnMut() -> bool,
    ) -> io::Result<Self> {
        Self::from_opened_file(
            open_regular_no_follow(path)?,
            true,
            Some(path.to_path_buf()),
            true,
            keep_going,
        )
    }

    /// Open an automatically discovered entry without following its final link
    /// and require the same filesystem object observed by the folder scan.
    #[cfg(test)]
    pub(crate) fn open_scanned(path: &Path, provenance: ScanProvenance) -> io::Result<Self> {
        Self::open_scanned_while(path, provenance, || true)
    }

    /// Open a scanned entry while cooperatively stopping superseded work.
    pub(crate) fn open_scanned_while(
        path: &Path,
        provenance: ScanProvenance,
        keep_going: impl FnMut() -> bool,
    ) -> io::Result<Self> {
        Self::open_scanned_with_witness_while(path, provenance, true, keep_going)
    }

    fn open_scanned_with_witness_while(
        path: &Path,
        provenance: ScanProvenance,
        capture_content_witness: bool,
        mut keep_going: impl FnMut() -> bool,
    ) -> io::Result<Self> {
        ensure_source_work_current(&mut keep_going)?;
        let file = open_regular_no_follow(path)?;
        let source = Self::from_opened_file(
            file,
            true,
            Some(path.to_path_buf()),
            capture_content_witness,
            &mut keep_going,
        )?;
        ensure_source_work_current(&mut keep_going)?;
        if source.identity != Some(provenance.identity)
            || source.version != Some(provenance.version)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "scanned image source changed before it was opened",
            ));
        }
        Ok(source)
    }

    /// Return scan-compatible identity evidence for this retained regular source.
    #[must_use]
    pub(crate) fn scan_provenance(&self) -> Option<ScanProvenance> {
        self.markable.then_some(())?;
        self.identity
            .zip(self.version)
            .map(|(identity, version)| ScanProvenance { identity, version })
    }

    /// Snapshot the retained handle's current identity and version. Restore
    /// uses this after a completed Trash round trip because rename operations
    /// legitimately change version evidence while preserving object identity.
    #[must_use]
    pub(crate) fn current_scan_provenance(&self) -> Option<ScanProvenance> {
        self.markable.then_some(())?;
        let metadata = self.file.metadata().ok()?;
        metadata_is_markable_regular(&metadata).then_some(())?;
        Some(ScanProvenance {
            identity: file_identity(&self.file, &metadata).ok()?,
            version: file_version(&self.file, &metadata).ok()?,
        })
    }

    /// Reopen the current regular pathname and require it to be this retained
    /// object, producing a fresh independent cursor and version snapshot.
    pub(crate) fn reopen_current_regular(&self, path: &Path) -> io::Result<Self> {
        let refreshed = Self::open_regular(path)?;
        if self.same_object(&refreshed) {
            Ok(refreshed)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "restored path does not name the retained source object",
            ))
        }
    }

    fn from_opened_file(
        file: std::fs::File,
        markable: bool,
        accepted_path: Option<PathBuf>,
        capture_content_witness: bool,
        mut keep_going: impl FnMut() -> bool,
    ) -> io::Result<Self> {
        ensure_source_work_current(&mut keep_going)?;
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
        if opened.len() > MAX_ACCEPTED_SOURCE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "image source exceeds the encoded input limit",
            ));
        }
        let identity = file_identity(&file, &opened).ok();
        let version = file_version(&file, &opened).ok();
        #[cfg(target_os = "windows")]
        let content_digest = if capture_content_witness {
            Some(sha256_file_while(
                &file,
                MAX_ACCEPTED_SOURCE_BYTES,
                &mut keep_going,
            )?)
        } else {
            None
        };
        #[cfg(not(target_os = "windows"))]
        let _ = capture_content_witness;
        ensure_source_work_current(&mut keep_going)?;
        Ok(Self {
            file,
            cursor: Mutex::new(()),
            identity,
            version,
            #[cfg(target_os = "windows")]
            content_digest,
            accepted_path,
            markable,
        })
    }

    /// Duplicate and rewind the accepted source while owning its shared cursor.
    pub(crate) fn clone_for_decode(&self) -> io::Result<ImageSourceReader<'_>> {
        let cursor = self
            .cursor
            .lock()
            .map_err(|_| io::Error::other("image source reader lock was poisoned"))?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(ImageSourceReader {
            file,
            _cursor: cursor,
        })
    }

    /// Whether all available version evidence still matches the accepted object.
    #[must_use]
    pub(crate) fn version_is_current(&self) -> bool {
        self.version_is_current_while(|| true)
    }

    /// Check version evidence while cooperatively stopping superseded work.
    #[must_use]
    pub(crate) fn version_is_current_while(&self, mut keep_going: impl FnMut() -> bool) -> bool {
        self.retained_version_is_current_while(&mut keep_going)
            && self.accepted_path.as_deref().is_none_or(|path| {
                !self.markable
                    || self.matches_path_inner(path, false, &mut keep_going)
                        == ImageSourceMatch::Same
            })
    }

    fn native_version_is_current_while(&self, keep_going: &mut impl FnMut() -> bool) -> bool {
        let Some(expected_version) = self.version else {
            return false;
        };
        if !keep_going() {
            return false;
        }
        let Ok(_cursor) = self.cursor.lock() else {
            return false;
        };
        let matches = self.file.metadata().is_ok_and(|metadata| {
            file_version(&self.file, &metadata).is_ok_and(|current| current == expected_version)
        });
        matches && keep_going()
    }

    #[cfg(not(target_os = "windows"))]
    fn retained_version_is_current_while(&self, keep_going: &mut impl FnMut() -> bool) -> bool {
        self.native_version_is_current_while(keep_going)
    }

    #[cfg(target_os = "windows")]
    fn retained_version_is_current_while(&self, keep_going: &mut impl FnMut() -> bool) -> bool {
        let Some(expected_version) = self.version else {
            return false;
        };
        let Some(expected_digest) = self.content_digest else {
            return false;
        };
        if !keep_going() {
            return false;
        }
        let Ok(_cursor) = self.cursor.lock() else {
            return false;
        };
        let version_matches = || {
            self.file.metadata().is_ok_and(|metadata| {
                file_version(&self.file, &metadata).is_ok_and(|current| current == expected_version)
            })
        };
        version_matches()
            && sha256_file_while(&self.file, MAX_ACCEPTED_SOURCE_BYTES, &mut *keep_going)
                .is_ok_and(|digest| digest == expected_digest)
            && version_matches()
            && keep_going()
    }

    /// Whether another retained source names the exact same filesystem object.
    #[must_use]
    pub(crate) fn same_object(&self, other: &Self) -> bool {
        self.identity
            .zip(other.identity)
            .is_some_and(|(left, right)| left == right)
    }

    #[cfg(test)]
    pub(crate) fn open_without_identity_for_test(path: &Path) -> io::Result<Self> {
        let mut source = Self::open(path)?;
        source.identity = None;
        source.version = None;
        Ok(source)
    }

    /// Compare this retained source with the current final pathname entry without
    /// following a link or accepting a non-regular object.
    #[must_use]
    pub(crate) fn matches_path(&self, path: &Path) -> ImageSourceMatch {
        self.matches_path_while(path, || true)
    }

    /// Compare only retained native identity and version evidence.
    ///
    /// This bounded check is suitable for non-mutating UI handoffs. Operations
    /// that publish or mutate accepted image bytes must use [`Self::matches_path`]
    /// from owned background work.
    #[must_use]
    pub(crate) fn matches_path_native(&self, path: &Path) -> ImageSourceMatch {
        self.matches_path_inner(path, false, &mut || true)
    }

    /// Compare a pathname while cooperatively stopping superseded work.
    #[must_use]
    pub(crate) fn matches_path_while(
        &self,
        path: &Path,
        mut keep_going: impl FnMut() -> bool,
    ) -> ImageSourceMatch {
        self.matches_path_inner(path, true, &mut keep_going)
    }

    fn matches_path_inner(
        &self,
        path: &Path,
        require_content_match: bool,
        keep_going: &mut impl FnMut() -> bool,
    ) -> ImageSourceMatch {
        if !keep_going() {
            return ImageSourceMatch::Unavailable;
        }
        if !self.markable {
            return ImageSourceMatch::Unsupported;
        }
        let Some(expected_identity) = self.identity else {
            return ImageSourceMatch::Unavailable;
        };
        let Some(expected_version) = self.version else {
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
        match (file_identity(&file, &opened), file_version(&file, &opened)) {
            (Ok(identity), Ok(version))
                if identity == expected_identity && version == expected_version => {}
            (Ok(_), Ok(_)) => return ImageSourceMatch::Changed,
            _ => return ImageSourceMatch::Unavailable,
        }
        if require_content_match {
            if !keep_going() {
                return ImageSourceMatch::Unavailable;
            }
            #[cfg(target_os = "windows")]
            match self.content_matches_while(&file, keep_going) {
                Ok(true) => {}
                Ok(false) => return ImageSourceMatch::Changed,
                Err(_) => return ImageSourceMatch::Unavailable,
            }
        }
        if !keep_going() {
            return ImageSourceMatch::Unavailable;
        }
        let refreshed = match file.metadata() {
            Ok(refreshed) if metadata_is_markable_regular(&refreshed) => refreshed,
            Ok(_) => return ImageSourceMatch::Unsupported,
            Err(_) => return ImageSourceMatch::Unavailable,
        };
        match (
            file_identity(&file, &refreshed),
            file_version(&file, &refreshed),
        ) {
            (Ok(identity), Ok(version))
                if identity == expected_identity && version == expected_version =>
            {
                ImageSourceMatch::Same
            }
            (Ok(_), Ok(_)) => ImageSourceMatch::Changed,
            _ => ImageSourceMatch::Unavailable,
        }
    }

    #[cfg(target_os = "windows")]
    fn content_matches_while(
        &self,
        file: &std::fs::File,
        keep_going: &mut impl FnMut() -> bool,
    ) -> io::Result<bool> {
        let expected_digest = self
            .content_digest
            .ok_or_else(|| io::Error::other("accepted source has no full content witness"))?;
        sha256_file_while(file, MAX_ACCEPTED_SOURCE_BYTES, keep_going)
            .map(|digest| digest == expected_digest)
    }

    /// Whether `path` currently names the retained filesystem object, ignoring
    /// version fields that can legitimately change when that object is renamed.
    ///
    /// Rating replacement uses this only for the backup pathname created by
    /// `ReplaceFileW`; source-path decisions continue to require `matches_path`.
    #[must_use]
    pub(crate) fn same_object_at_path(&self, path: &Path) -> bool {
        if !self.markable {
            return false;
        }
        let Some(expected_identity) = self.identity else {
            return false;
        };
        let Ok(entry) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if !metadata_is_markable_regular(&entry) {
            return false;
        }
        let Ok(file) = open_regular_no_follow(path) else {
            return false;
        };
        let Ok(opened) = file.metadata() else {
            return false;
        };
        metadata_is_markable_regular(&opened)
            && file_identity(&file, &opened).is_ok_and(|identity| identity == expected_identity)
    }
}

impl RatingScanSource {
    /// Open a read-only rating source, retaining scan provenance when available.
    pub(crate) fn open_while(
        path: &Path,
        provenance: Option<ScanProvenance>,
        keep_going: impl FnMut() -> bool,
    ) -> io::Result<Self> {
        let source = if let Some(provenance) = provenance {
            ImageSource::open_scanned_with_witness_while(path, provenance, false, keep_going)?
        } else {
            ImageSource::open_with_witness_while(path, false, keep_going)?
        };
        Ok(Self(source))
    }

    /// Duplicate and rewind the retained source for bounded header inspection.
    pub(crate) fn clone_for_read(&self) -> io::Result<ImageSourceReader<'_>> {
        self.0.clone_for_decode()
    }

    /// Validate native identity and version evidence without granting write authority.
    #[must_use]
    pub(crate) fn native_version_is_current_while(
        &self,
        mut keep_going: impl FnMut() -> bool,
    ) -> bool {
        self.0.native_version_is_current_while(&mut keep_going)
            && self.0.accepted_path.as_deref().is_none_or(|path| {
                !self.0.markable
                    || self.0.matches_path_inner(path, false, &mut keep_going)
                        == ImageSourceMatch::Same
            })
    }
}

impl PathIdentitySource {
    /// Compare the destination pathname using native identity/version evidence.
    #[must_use]
    pub(crate) fn matches_path(&self, path: &Path) -> ImageSourceMatch {
        self.0.matches_path_native(path)
    }

    /// Whether this destination is the exact accepted source filesystem object.
    #[must_use]
    pub(crate) fn same_object(&self, source: &ImageSource) -> bool {
        self.0.same_object(source)
    }
}

impl DirectorySource {
    /// Open a canonical directory without following its final component.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let entry = std::fs::symlink_metadata(path)?;
        if !metadata_is_plain_directory(&entry) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Save As parent must be an ordinary directory",
            ));
        }
        let file = open_directory_no_follow(path)?;
        let opened = file.metadata()?;
        if !metadata_is_plain_directory(&opened) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Save As parent changed while it was opened",
            ));
        }
        let identity = file_identity(&file, &opened)?;
        Ok(Self { file, identity })
    }

    fn open_regular(&self, name: &OsStr) -> io::Result<std::fs::File> {
        open_regular_at(&self.file, name)
    }

    /// Open a retained regular destination identity without following its final
    /// entry, re-resolving the parent pathname, or hashing its contents.
    pub(crate) fn open_image_identity(&self, name: &OsStr) -> io::Result<PathIdentitySource> {
        PathIdentitySource::from_opened_file(self.open_regular(name)?)
    }

    /// Whether `path` still resolves to the retained directory object.
    #[must_use]
    pub(crate) fn matches_path(&self, path: &Path) -> bool {
        let Ok(entry) = std::fs::symlink_metadata(path) else {
            return false;
        };
        if !metadata_is_plain_directory(&entry) {
            return false;
        }
        let Ok(file) = open_directory_no_follow(path) else {
            return false;
        };
        let Ok(opened) = file.metadata() else {
            return false;
        };
        metadata_is_plain_directory(&opened)
            && file_identity(&file, &opened).is_ok_and(|identity| identity == self.identity)
    }
}

/// Whether `path` still names the exact retained regular file without
/// following its final component.
#[must_use]
pub(crate) fn regular_path_matches_file(path: &Path, retained: &std::fs::File) -> bool {
    let Ok(retained_metadata) = retained.metadata() else {
        return false;
    };
    if !metadata_is_markable_regular(&retained_metadata) {
        return false;
    }
    let Ok(candidate) = open_regular_no_follow(path) else {
        return false;
    };
    let Ok(candidate_metadata) = candidate.metadata() else {
        return false;
    };
    metadata_is_markable_regular(&candidate_metadata)
        && matches!(
            (
                file_identity(retained, &retained_metadata),
                file_identity(&candidate, &candidate_metadata)
            ),
            (Ok(expected), Ok(actual)) if expected == actual
        )
}

#[cfg(target_os = "windows")]
fn metadata_is_markable_regular(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_file() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(target_os = "windows")]
fn metadata_is_plain_directory(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_markable_regular(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(not(target_os = "windows"))]
fn metadata_is_plain_directory(metadata: &std::fs::Metadata) -> bool {
    metadata.is_dir()
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
        let base_flags = libc::O_NOFOLLOW | libc::O_NONBLOCK;
        open_with(base_flags | libc::O_NOATIME).or_else(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                open_with(base_flags)
            } else {
                Err(error)
            }
        })
    }

    #[cfg(not(target_os = "linux"))]
    {
        open_with(libc::O_NOFOLLOW | libc::O_NONBLOCK)
    }
}

#[cfg(unix)]
fn open_directory_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    options.open(path)
}

#[cfg(unix)]
#[allow(unsafe_code)] // one audited handle-relative, no-follow folder-entry open
fn open_regular_at(directory: &std::fs::File, name: &OsStr) -> io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(name.as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "folder entry name contains an interior NUL",
        )
    })?;
    let open_with = |flags| {
        // SAFETY: `directory` owns a live directory descriptor, `name` is a
        // NUL-terminated single entry name from `read_dir`, and a successful
        // descriptor is transferred immediately into `File` ownership.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if descriptor < 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: `openat` returned a new owned descriptor on success.
            Ok(unsafe { std::fs::File::from_raw_fd(descriptor) })
        }
    };
    let base_flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    #[cfg(target_os = "linux")]
    {
        open_with(base_flags | libc::O_NOATIME).or_else(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                open_with(base_flags)
            } else {
                Err(error)
            }
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        open_with(base_flags)
    }
}

/// Atomically replace an existing Windows file, optionally retaining a backup.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited failure-atomic Windows replacement boundary
pub(crate) fn replace_file(
    target: &Path,
    replacement: &Path,
    backup: Option<&Path>,
) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    let target = target
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let replacement = replacement
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let backup = backup.map(|path| {
        path.as_os_str()
            .encode_wide()
            .chain(iter::once(0))
            .collect::<Vec<_>>()
    });
    let backup_pointer = backup.as_ref().map_or(std::ptr::null(), Vec::as_ptr);
    // SAFETY: All three paths are stable, NUL-terminated UTF-16 buffers for the
    // duration of the call. Flags are zero, and no reserved pointers are used.
    let succeeded = unsafe {
        ReplaceFileW(
            target.as_ptr(),
            replacement.as_ptr(),
            backup_pointer,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
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

#[cfg(target_os = "windows")]
fn open_directory_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    options.open(path)
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited handle-relative, no-follow folder-entry open
fn open_regular_at(directory: &std::fs::File, name: &OsStr) -> io::Result<std::fs::File> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
        NtCreateFile,
    };
    use windows_sys::Win32::Foundation::{HANDLE, RtlNtStatusToDosError, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut name = name.encode_wide().collect::<Vec<_>>();
    let byte_length = name
        .len()
        .checked_mul(size_of::<u16>())
        .and_then(|length| u16::try_from(length).ok())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "folder entry name is too long")
        })?;
    let unicode_name = UNICODE_STRING {
        Length: byte_length,
        MaximumLength: byte_length,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>())
            .map_err(|_| io::Error::other("object attribute structure is too large"))?,
        RootDirectory: directory.as_raw_handle() as HANDLE,
        ObjectName: &raw const unicode_name,
        Attributes: 0x40,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle: HANDLE = std::ptr::null_mut();
    let mut status_block = MaybeUninit::<IO_STATUS_BLOCK>::zeroed();
    // SAFETY: The directory handle, UTF-16 name, object attributes, output
    // handle, and status storage remain valid for the synchronous call. The
    // root handle scopes resolution to the enumerated directory object, and
    // the create options open neither directories nor reparse targets.
    let status = unsafe {
        NtCreateFile(
            &raw mut handle,
            FILE_GENERIC_READ,
            &raw const attributes,
            status_block.as_mut_ptr(),
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        // SAFETY: Converting an NTSTATUS returned by `NtCreateFile` is a pure
        // platform error-code translation.
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(
            i32::try_from(code).unwrap_or(i32::MAX),
        ));
    }
    if handle.is_null() {
        return Err(io::Error::other(
            "handle-relative folder entry open returned no handle",
        ));
    }
    // SAFETY: A successful `NtCreateFile` returned a new owned file handle.
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
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

#[cfg(unix)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "shared platform version boundary is fallible on Windows and callers handle one Result contract"
)]
fn file_version(_file: &std::fs::File, metadata: &std::fs::Metadata) -> io::Result<FileVersion> {
    use std::os::unix::fs::MetadataExt;

    Ok(FileVersion {
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    })
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)] // one audited read-only Win32 file-version query
fn file_version(file: &std::fs::File, metadata: &std::fs::Metadata) -> io::Result<FileVersion> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx,
    };

    let mut info = MaybeUninit::<FILE_BASIC_INFO>::uninit();
    let size = u32::try_from(size_of::<FILE_BASIC_INFO>())
        .map_err(|_| io::Error::other("file version structure is too large"))?;
    // SAFETY: `file` owns a valid handle for the call. `info` provides exactly
    // `size` writable bytes, and the API retains no pointer.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: A successful call initialized the complete FILE_BASIC_INFO value.
    let info = unsafe { info.assume_init() };
    Ok(FileVersion {
        length: metadata.len(),
        last_write_time: info.LastWriteTime,
        change_time: info.ChangeTime,
    })
}

#[cfg(all(target_os = "windows", test))]
fn sha256_file(file: &std::fs::File) -> io::Result<[u8; SHA256_BYTES]> {
    sha256_file_while(file, MAX_ACCEPTED_SOURCE_BYTES, || true)
}

#[cfg(target_os = "windows")]
fn sha256_file_while(
    file: &std::fs::File,
    max_bytes: u64,
    mut keep_going: impl FnMut() -> bool,
) -> io::Result<[u8; SHA256_BYTES]> {
    ensure_source_work_current(&mut keep_going)?;
    let declared_bytes = file.metadata()?.len();
    if declared_bytes > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "image source exceeds the content-witness limit",
        ));
    }
    let algorithm = Sha256Algorithm::open()?;
    let hash = Sha256Hash::new(&algorithm)?;
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let digest = (|| {
        let mut buffer = vec![0_u8; CONTENT_WITNESS_CHUNK_BYTES];
        let mut remaining = declared_bytes;
        while remaining != 0 {
            ensure_source_work_current(&mut keep_going)?;
            let buffer_bytes = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
            let chunk = usize::try_from(remaining.min(buffer_bytes))
                .map_err(|_| io::Error::other("content-witness chunk is not representable"))?;
            let read = reader.read(&mut buffer[..chunk])?;
            ensure_source_work_current(&mut keep_going)?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "image source shrank while its content witness was computed",
                ));
            }
            hash.update(&buffer[..read])?;
            let read = u64::try_from(read)
                .map_err(|_| io::Error::other("content-witness read is not representable"))?;
            remaining = remaining
                .checked_sub(read)
                .ok_or_else(|| io::Error::other("content-witness read exceeded its bound"))?;
        }
        ensure_source_work_current(&mut keep_going)?;
        let mut extra = [0_u8; 1];
        if reader.read(&mut extra)? != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "image source grew while its content witness was computed",
            ));
        }
        ensure_source_work_current(&mut keep_going)?;
        hash.finish()
    })();
    let rewind = reader.seek(SeekFrom::Start(0));
    match (digest, rewind) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(digest), Ok(_)) => Ok(digest),
    }
}

fn ensure_source_work_current(keep_going: &mut impl FnMut() -> bool) -> io::Result<()> {
    if keep_going() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "image source work was superseded",
        ))
    }
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
/// Listed so folder navigation includes them. AVIF/HEIC success depends on a
/// co-located worker binary and its Cargo features. Camera RAW stays a
/// documented error through 1.0 (see `docs/FORMATS.md`).
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
    scan_images_with_limit(directory, MAX_FOLDER_IMAGES, MAX_FOLDER_PATH_BYTES, || true).map_err(
        |error| match error {
            ScanImagesError::Io(error) => error,
            ScanImagesError::Cancelled => {
                io::Error::new(io::ErrorKind::Interrupted, "folder scan was superseded")
            }
            ScanImagesError::LimitExceeded => {
                io::Error::new(io::ErrorKind::InvalidData, "folder image limit exceeded")
            }
            ScanImagesError::PathBudgetExceeded => io::Error::new(
                io::ErrorKind::InvalidData,
                "folder path-data safety limit exceeded",
            ),
            ScanImagesError::WorkerStopped => {
                io::Error::other("folder scan worker stopped unexpectedly")
            }
        },
    )
}

#[cfg(test)]
pub(crate) fn scan_images_while(
    directory: &Path,
    keep_going: impl FnMut() -> bool,
) -> Result<Vec<PathBuf>, ScanImagesError> {
    scan_images_with_limit(
        directory,
        MAX_FOLDER_IMAGES,
        MAX_FOLDER_PATH_BYTES,
        keep_going,
    )
}

pub(crate) fn scan_image_entries_while(
    directory: &Path,
    keep_going: impl FnMut() -> bool,
) -> Result<Vec<ScannedImage>, ScanImagesError> {
    scan_image_entries_with_limit(
        directory,
        MAX_FOLDER_IMAGES,
        MAX_FOLDER_PATH_BYTES,
        keep_going,
    )
}

fn scan_images_with_limit(
    directory: &Path,
    max_files: usize,
    max_path_bytes: usize,
    keep_going: impl FnMut() -> bool,
) -> Result<Vec<PathBuf>, ScanImagesError> {
    scan_image_entries_with_limit(directory, max_files, max_path_bytes, keep_going)
        .map(|entries| entries.into_iter().map(|entry| entry.path).collect())
}

fn scan_image_entries_with_limit(
    directory: &Path,
    max_files: usize,
    max_path_bytes: usize,
    keep_going: impl FnMut() -> bool,
) -> Result<Vec<ScannedImage>, ScanImagesError> {
    scan_image_entries_with_hook(directory, max_files, max_path_bytes, keep_going, || {})
}

fn scan_image_entries_with_hook(
    directory: &Path,
    max_files: usize,
    max_path_bytes: usize,
    mut keep_going: impl FnMut() -> bool,
    after_directory_open: impl FnOnce(),
) -> Result<Vec<ScannedImage>, ScanImagesError> {
    if !keep_going() {
        return Err(ScanImagesError::Cancelled);
    }
    let directory = directory.canonicalize().map_err(ScanImagesError::Io)?;
    let directory_source = DirectorySource::open(&directory).map_err(ScanImagesError::Io)?;
    after_directory_open();
    let entries = std::fs::read_dir(&directory).map_err(ScanImagesError::Io)?;
    if !directory_source.matches_path(&directory) {
        return Err(ScanImagesError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "folder changed while its entries were opened",
        )));
    }
    let mut files = Vec::new();
    let mut path_bytes = 0_usize;
    for entry in entries {
        if !keep_going() {
            return Err(ScanImagesError::Cancelled);
        }
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !is_supported_image(&path) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(file) = directory_source.open_regular(&entry.file_name()) else {
            continue;
        };
        let Ok(opened) = file.metadata() else {
            continue;
        };
        if !metadata_is_markable_regular(&opened) {
            continue;
        }
        let Ok(identity) = file_identity(&file, &opened) else {
            continue;
        };
        let Ok(version) = file_version(&file, &opened) else {
            continue;
        };
        if files.len() == max_files {
            return Err(ScanImagesError::LimitExceeded);
        }
        path_bytes = path_bytes
            .checked_add(path.as_os_str().as_encoded_bytes().len())
            .filter(|bytes| *bytes <= max_path_bytes)
            .ok_or(ScanImagesError::PathBudgetExceeded)?;
        files.push(ScannedImage {
            path,
            provenance: ScanProvenance { identity, version },
        });
    }
    if !keep_going() {
        return Err(ScanImagesError::Cancelled);
    }
    let completed_sort = sort_while(&mut files, &mut keep_going, |a, b| {
        let a_name = a
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let b_name = b
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        natural_cmp(a_name, b_name)
    });
    if !completed_sort || !keep_going() {
        return Err(ScanImagesError::Cancelled);
    }
    Ok(files)
}

fn sort_while<T>(
    values: &mut [T],
    keep_going: &mut impl FnMut() -> bool,
    mut compare: impl FnMut(&T, &T) -> Ordering,
) -> bool {
    let mut cancelled = false;
    values.sort_by(|left, right| {
        if cancelled || !keep_going() {
            cancelled = true;
            Ordering::Equal
        } else {
            compare(left, right)
        }
    });
    !cancelled
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
    #[cfg(unix)]
    use super::scan_image_entries_with_hook;
    use super::{
        CORE_EXTENSIONS, CORE_MIME_ASSOCIATIONS, canonical_file_path, is_core_format,
        is_supported_image, is_worker_format, natural_cmp, scan_image_entries_while, scan_images,
        scan_images_while, scan_images_with_limit, sort_while, supported_extensions,
    };
    use crate::ephemeral::TempWorkspace;
    use std::cmp::Ordering;
    use std::fs;
    use std::io::Read;
    #[cfg(windows)]
    use std::io::{Seek, SeekFrom, Write};
    use std::path::Path;
    use std::sync::{Arc, Barrier};

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
    fn scanned_entry_rejects_an_in_place_rewrite() {
        let workspace = TempWorkspace::new("folder_scan_version").unwrap();
        let path = workspace.path().join("image.png");
        fs::write(&path, b"scanned object").unwrap();
        let entry = scan_image_entries_while(workspace.path(), || true)
            .unwrap()
            .pop()
            .unwrap();
        let (scanned_path, provenance) = entry.into_parts();

        fs::write(&path, b"rewritten object with a different length").unwrap();

        assert!(super::ImageSource::open_scanned(&scanned_path, provenance).is_err());
    }

    #[test]
    fn restored_handle_can_capture_fresh_version_bound_provenance() {
        let workspace = TempWorkspace::new("restored_source_version").unwrap();
        let path = workspace.path().join("image.png");
        let trashed = workspace.path().join("trashed.png");
        fs::write(&path, b"restored object").unwrap();
        let source = super::ImageSource::open_regular(&path).unwrap();
        let original_provenance = source.scan_provenance().unwrap();
        fs::rename(&path, &trashed).unwrap();
        fs::rename(&trashed, &path).unwrap();
        let modified = fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .checked_add(std::time::Duration::from_hours(24))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();

        assert!(super::ImageSource::open_scanned(&path, original_provenance).is_err());
        let restored_provenance = source.current_scan_provenance().unwrap();
        assert!(super::ImageSource::open_scanned(&path, restored_provenance).is_ok());
        let refreshed = source.reopen_current_regular(&path).unwrap();
        assert!(refreshed.version_is_current());
        assert!(source.same_object(&refreshed));
    }

    #[cfg(unix)]
    #[test]
    #[allow(unsafe_code)] // creates one local FIFO fixture through the POSIX API
    fn no_follow_regular_opens_reject_a_fifo_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::time::{Duration, Instant};

        let workspace = TempWorkspace::new("regular_open_fifo").unwrap();
        let path = workspace.path().join("image.png");
        let raw_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `raw_path` is a live NUL-terminated local test path and the
        // mode grants access only to the current user.
        assert_eq!(unsafe { libc::mkfifo(raw_path.as_ptr(), 0o600) }, 0);
        let directory = super::DirectorySource::open(workspace.path()).unwrap();

        let started = Instant::now();
        assert!(super::ImageSource::open_regular(&path).is_err());
        assert!(
            directory
                .open_image_identity(path.file_name().unwrap())
                .is_err()
        );

        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn scanned_entry_rejects_a_later_final_symlink_substitution() {
        use std::os::unix::fs::symlink;

        let workspace = TempWorkspace::new("folder_scan_child_swap").unwrap();
        let outside_workspace = TempWorkspace::new("folder_scan_child_swap_outside").unwrap();
        let path = workspace.path().join("image.png");
        let outside = outside_workspace.path().join("outside.png");
        fs::write(&path, b"scanned object").unwrap();
        fs::write(&outside, b"outside object").unwrap();
        let entry = scan_image_entries_while(workspace.path(), || true)
            .unwrap()
            .pop()
            .unwrap();
        let (scanned_path, provenance) = entry.into_parts();

        fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();

        assert!(super::ImageSource::open_scanned(&scanned_path, provenance).is_err());
        let explicit_source = super::ImageSource::open(&scanned_path).unwrap();
        let mut explicit = explicit_source.clone_for_decode().unwrap();
        let mut bytes = Vec::new();
        explicit.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"outside object");
    }

    #[cfg(unix)]
    #[test]
    fn scan_identity_stays_bound_to_the_retained_directory_after_rebind() {
        let workspace = TempWorkspace::new("folder_scan_parent_rebind").unwrap();
        let selected = workspace.path().join("selected");
        let moved = workspace.path().join("moved");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("image.png"), b"original object").unwrap();

        let error = scan_image_entries_with_hook(
            &selected,
            2,
            usize::MAX,
            || true,
            || {
                fs::rename(&selected, &moved).unwrap();
                fs::create_dir(&selected).unwrap();
                fs::write(selected.join("image.png"), b"replacement object").unwrap();
            },
        )
        .err()
        .expect("the rebound directory must invalidate its retained scan");

        assert_eq!(
            error.to_string(),
            "folder changed while its entries were opened"
        );
    }

    #[test]
    fn scan_reports_an_unopenable_directory() {
        let workspace = TempWorkspace::new("folder_scan_file").unwrap();
        let file = workspace.path().join("image.png");
        fs::write(&file, b"fixture").unwrap();
        assert!(scan_images(&file).is_err());
    }

    #[test]
    fn bounded_scan_rejects_excess_images_instead_of_truncating() {
        let workspace = TempWorkspace::new("folder_scan_limit").unwrap();
        for name in ["one.png", "two.jpg", "three.webp"] {
            fs::write(workspace.path().join(name), b"fixture").unwrap();
        }

        let error = scan_images_with_limit(workspace.path(), 2, usize::MAX, || true).unwrap_err();
        assert!(matches!(error, super::ScanImagesError::LimitExceeded));
    }

    #[test]
    fn bounded_scan_rejects_cumulative_deep_parent_path_storage() {
        let workspace = TempWorkspace::new("folder_scan_path_budget").unwrap();
        let directory = workspace
            .path()
            .join("deep-parent-component")
            .join("another-deep-component")
            .join("third-deep-component");
        fs::create_dir_all(&directory).unwrap();
        let paths = ["one.png", "two.jpg", "three.webp"].map(|name| directory.join(name));
        for path in &paths {
            fs::write(path, b"fixture").unwrap();
        }
        let two_path_budget = paths[..2]
            .iter()
            .map(|path| path.as_os_str().as_encoded_bytes().len())
            .sum();

        let error = scan_images_with_limit(&directory, 3, two_path_budget, || true).unwrap_err();

        assert!(matches!(error, super::ScanImagesError::PathBudgetExceeded));
    }

    #[test]
    fn cancelled_scan_stops_before_opening_the_directory() {
        let error = scan_images_while(Path::new("must-not-be-opened"), || false).unwrap_err();
        assert!(matches!(error, super::ScanImagesError::Cancelled));
    }

    #[test]
    fn scan_observes_cancellation_during_and_after_enumeration() {
        let workspace = TempWorkspace::new("folder_scan_cancellation_points").unwrap();
        fs::write(workspace.path().join("one.png"), b"fixture").unwrap();

        let mut checks = 0;
        let during = scan_images_with_limit(workspace.path(), 2, usize::MAX, || {
            checks += 1;
            checks == 1
        });
        assert!(matches!(during, Err(super::ScanImagesError::Cancelled)));

        fs::remove_file(workspace.path().join("one.png")).unwrap();
        let mut checks = 0;
        let before_sort = scan_images_with_limit(workspace.path(), 2, usize::MAX, || {
            checks += 1;
            checks < 2
        });
        assert!(matches!(
            before_sort,
            Err(super::ScanImagesError::Cancelled)
        ));

        let mut checks = 0;
        let after_sort = scan_images_with_limit(workspace.path(), 2, usize::MAX, || {
            checks += 1;
            checks < 3
        });
        assert!(matches!(after_sort, Err(super::ScanImagesError::Cancelled)));
    }

    #[test]
    fn scan_observes_cancellation_during_natural_sort() {
        let workspace = TempWorkspace::new("folder_scan_sort_cancellation").unwrap();
        fs::write(workspace.path().join("two.png"), b"fixture").unwrap();
        fs::write(workspace.path().join("one.png"), b"fixture").unwrap();
        let mut checks = 0;

        let result = scan_images_with_limit(workspace.path(), 2, usize::MAX, || {
            checks += 1;
            checks != 5
        });

        assert!(matches!(result, Err(super::ScanImagesError::Cancelled)));
    }

    #[test]
    fn cancelled_sort_stops_all_remaining_comparisons() {
        let mut values = [6, 5, 4, 3, 2, 1];
        let mut cancellation_checks = 0_usize;
        let mut comparisons = 0_usize;

        let completed = sort_while(
            &mut values,
            &mut || {
                cancellation_checks += 1;
                cancellation_checks < 3
            },
            |left, right| {
                comparisons += 1;
                left.cmp(right)
            },
        );

        assert!(!completed);
        assert_eq!(cancellation_checks, 3);
        assert_eq!(comparisons, 2);
    }

    #[test]
    fn scan_errors_have_fixed_specific_messages() {
        let cases = [
            (
                super::ScanImagesError::Io(std::io::Error::other("read failed")),
                "read failed",
            ),
            (
                super::ScanImagesError::Cancelled,
                "folder scan was superseded",
            ),
            (
                super::ScanImagesError::LimitExceeded,
                "folder image limit exceeded",
            ),
            (
                super::ScanImagesError::PathBudgetExceeded,
                "folder path-data safety limit exceeded",
            ),
            (
                super::ScanImagesError::WorkerStopped,
                "folder scan worker stopped unexpectedly",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn concurrent_source_readers_each_observe_the_complete_accepted_bytes() {
        let workspace = TempWorkspace::new("source_reader_serialization").unwrap();
        let path = workspace.path().join("source.bin");
        let expected = Arc::new(
            (0..512 * 1024)
                .map(|index| u8::try_from(index % 251).unwrap())
                .collect::<Vec<_>>(),
        );
        fs::write(&path, expected.as_slice()).unwrap();
        let source = Arc::new(super::ImageSource::open(&path).unwrap());
        let barrier = Arc::new(Barrier::new(5));
        let mut workers = Vec::new();

        for _ in 0..4 {
            let source = Arc::clone(&source);
            let expected = Arc::clone(&expected);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let mut reader = source.clone_for_decode().unwrap();
                let mut actual = Vec::with_capacity(expected.len());
                let mut chunk = [0_u8; 4096];
                loop {
                    let read = reader.read(&mut chunk).unwrap();
                    if read == 0 {
                        break;
                    }
                    actual.extend_from_slice(&chunk[..read]);
                    std::thread::yield_now();
                }
                actual == *expected
            }));
        }

        barrier.wait();
        for worker in workers {
            assert!(worker.join().unwrap());
        }
    }

    #[test]
    fn source_version_rejects_path_replacement_with_unchanged_retained_metadata() {
        let workspace = TempWorkspace::new("source_path_replacement").unwrap();
        let path = workspace.path().join("source.bin");
        let displaced = workspace.path().join("displaced.bin");
        fs::write(&path, b"accepted-bytes").unwrap();
        let mut source = super::ImageSource::open(&path).unwrap();

        fs::rename(&path, &displaced).unwrap();
        fs::write(&path, b"replaced-bytes").unwrap();
        let retained_metadata = source.file.metadata().unwrap();
        source.version = Some(super::file_version(&source.file, &retained_metadata).unwrap());

        assert!(!source.version_is_current());
        assert_eq!(source.matches_path(&path), super::ImageSourceMatch::Changed);
    }

    #[test]
    fn source_version_rejects_same_length_rewrite_with_restored_modified_time() {
        let workspace = TempWorkspace::new("source_version_change_time").unwrap();
        let path = workspace.path().join("source.bin");
        fs::write(&path, b"accepted-bytes").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let source = super::ImageSource::open(&path).unwrap();

        fs::write(&path, b"replaced-bytes").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();

        assert!(!source.version_is_current());
        assert_eq!(source.matches_path(&path), super::ImageSourceMatch::Changed);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn strong_match_rejects_rewritten_content_after_native_evidence_is_rebased() {
        let workspace = TempWorkspace::new("source_content_witness").unwrap();
        let path = workspace.path().join("source.bin");
        fs::write(&path, b"accepted-bytes").unwrap();
        let mut source = super::ImageSource::open(&path).unwrap();

        fs::write(&path, b"replaced-bytes").unwrap();
        let retained_metadata = source.file.metadata().unwrap();
        source.version = Some(super::file_version(&source.file, &retained_metadata).unwrap());

        assert_eq!(
            source.matches_path_native(&path),
            super::ImageSourceMatch::Same
        );
        assert_eq!(source.matches_path(&path), super::ImageSourceMatch::Changed);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn destination_identity_does_not_capture_a_content_witness() {
        let workspace = TempWorkspace::new("destination_identity").unwrap();
        let path = workspace.path().join("destination.png");
        fs::write(&path, b"existing destination").unwrap();
        let directory = super::DirectorySource::open(workspace.path()).unwrap();

        let destination = directory
            .open_image_identity(path.file_name().unwrap())
            .unwrap();

        assert!(destination.0.content_digest.is_none());
        assert_eq!(
            destination.matches_path(&path),
            super::ImageSourceMatch::Same
        );
    }

    #[test]
    fn destination_identity_does_not_inherit_the_source_size_limit() {
        let workspace = TempWorkspace::new("destination_identity_size").unwrap();
        let path = workspace.path().join("destination.png");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(super::MAX_ACCEPTED_SOURCE_BYTES + 1).unwrap();
        drop(file);
        let directory = super::DirectorySource::open(workspace.path()).unwrap();

        let destination = directory
            .open_image_identity(path.file_name().unwrap())
            .unwrap();

        assert_eq!(
            destination.matches_path(&path),
            super::ImageSourceMatch::Same
        );
    }

    #[test]
    fn source_open_rejects_a_sparse_file_above_the_encoded_input_limit() {
        let workspace = TempWorkspace::new("source_encoded_limit").unwrap();
        let path = workspace.path().join("source.bin");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(super::MAX_ACCEPTED_SOURCE_BYTES + 1).unwrap();
        drop(file);

        let error = super::ImageSource::open(&path).err().unwrap();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sha256_matches_the_standard_vector_and_rewinds() {
        let workspace = TempWorkspace::new("sha256_vector").unwrap();
        let path = workspace.path().join("source.bin");
        fs::write(&path, b"abc").unwrap();
        let mut file = std::fs::File::open(path).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();

        assert_eq!(
            super::sha256_file(&file).unwrap(),
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]
        );
        assert_eq!(file.stream_position().unwrap(), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sha256_stops_between_bounded_chunks_and_rewinds() {
        let workspace = TempWorkspace::new("sha256_cancel").unwrap();
        let path = workspace.path().join("source.bin");
        fs::write(&path, vec![0x5a; 3 * super::CONTENT_WITNESS_CHUNK_BYTES]).unwrap();
        let mut file = std::fs::File::open(path).unwrap();
        let mut checks = 0_u8;

        let error = super::sha256_file_while(&file, u64::MAX, || {
            checks = checks.saturating_add(1);
            checks < 4
        })
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(checks, 4);
        assert_eq!(file.stream_position().unwrap(), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_sha256_rejects_growth_past_the_declared_bound_and_rewinds() {
        let workspace = TempWorkspace::new("sha256_growth").unwrap();
        let path = workspace.path().join("source.bin");
        fs::write(&path, b"abc").unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        let mut writer = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        let mut checks = 0_u8;

        let error = super::sha256_file_while(&file, 3, || {
            checks = checks.saturating_add(1);
            if checks == 4 {
                writer.write_all(b"d").unwrap();
                writer.flush().unwrap();
            }
            true
        })
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(checks, 4);
        assert_eq!(file.stream_position().unwrap(), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn rating_scan_source_cannot_pass_a_full_content_witness_check() {
        let workspace = TempWorkspace::new("rating_scan_capability").unwrap();
        let path = workspace.path().join("source.jpg");
        fs::write(&path, b"rating header bytes").unwrap();
        let source = super::RatingScanSource::open_while(&path, None, || true).unwrap();

        assert!(source.native_version_is_current_while(|| true));
        assert!(!source.0.version_is_current());
        assert_eq!(
            source.0.matches_path(&path),
            super::ImageSourceMatch::Unavailable
        );
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

    #[test]
    fn directory_stamp_changes_when_a_child_is_added() {
        let workspace = TempWorkspace::new("directory_stamp").unwrap();
        let first = super::directory_stamp(workspace.path()).expect("stamp empty folder");
        fs::write(workspace.path().join("added.png"), b"fixture").unwrap();
        let second = super::directory_stamp(workspace.path()).expect("stamp after add");
        assert!(first.identity == second.identity);
        assert!(first != second);
        fs::remove_file(workspace.path().join("added.png")).unwrap();
        let third = super::directory_stamp(workspace.path()).expect("stamp after remove");
        assert!(first.identity == third.identity);
        assert!(second != third);
    }
}
