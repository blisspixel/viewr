//! Narrow macOS integration for Launch Services and recoverable trash.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2::ffi;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::{NSApplication, NSModalResponseOK, NSOpenPanel, NSWorkspace};
use objc2_foundation::{MainThreadMarker, NSArray, NSFileManager, NSString, NSURL};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;

static OPEN_FILE_PROXY: OnceLock<EventLoopProxy<UserEvent>> = OnceLock::new();

type OpenUrlsMethod = unsafe extern "C-unwind" fn(&AnyObject, Sel, &NSApplication, &NSArray<NSURL>);

/// Add Launch Services delivery to winit's existing application delegate.
///
/// Winit owns its delegate and stores event-loop state in that object. Replacing
/// it would prevent startup callbacks and later panic inside winit, so this
/// integration adds only the missing selector to the exact delegate class that
/// winit 0.30 registers.
pub(crate) fn install_open_file_handler(
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), &'static str> {
    let mtm = MainThreadMarker::new().ok_or("macOS event loop is not on the main thread")?;
    let application = NSApplication::sharedApplication(mtm);
    let delegate = application
        .delegate()
        .ok_or("winit did not register its macOS application delegate")?;
    let object: &AnyObject = AsRef::<AnyObject>::as_ref(&*delegate);
    let class = object.class();
    if class.name() != c"WinitApplicationDelegate" {
        return Err("the macOS application delegate is not owned by winit 0.30");
    }

    let selector = sel!(application:openURLs:);
    if class.instance_method(selector).is_some() {
        return Err("winit's macOS application delegate already handles open URLs");
    }
    if OPEN_FILE_PROXY.set(proxy).is_err() {
        return Err("the macOS open-file handler was already installed");
    }

    // SAFETY: Objective-C stores method implementations as erased function
    // pointers. The concrete callback signature is validated against the
    // selector and type encoding in the `class_addMethod` safety argument.
    let implementation: Imp =
        unsafe { std::mem::transmute(application_open_urls as OpenUrlsMethod) };
    // SAFETY: `class` is the registered winit delegate class and remains valid
    // for the process lifetime. The implementation signature and `v@:@@` type
    // encoding exactly match `application:openURLs:`. No existing method is
    // replaced, as checked above.
    let added = unsafe {
        ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            implementation,
            c"v@:@@".as_ptr(),
        )
    };
    if !added.as_bool() {
        return Err("could not extend winit's macOS delegate with open-file delivery");
    }
    Ok(())
}

unsafe extern "C-unwind" fn application_open_urls(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: &NSApplication,
    urls: &NSArray<NSURL>,
) {
    let path = first_core_path(urls);
    if let (Some(proxy), Some(path)) = (OPEN_FILE_PROXY.get(), path) {
        let _ = proxy.send_event(UserEvent::OpenFile(path));
    }
}

fn first_core_path(urls: &NSArray<NSURL>) -> Option<PathBuf> {
    urls.iter()
        .filter_map(|url| file_url_path(&url))
        .find(|path| crate::fs::is_core_format(path))
}

/// Move an image to Trash and retain the exact resulting path for Undo.
pub(crate) fn move_to_trash(path: &Path) -> Result<PathBuf, String> {
    objc2::rc::autoreleasepool(|_| {
        let source = file_url(path)?;
        let mut resulting_url = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&source, Some(&mut resulting_url))
            .map_err(|_| "macOS Trash rejected the file".to_owned())?;
        let resulting_url = resulting_url.ok_or("macOS did not return the trashed item URL")?;
        file_url_path(&resulting_url).ok_or_else(|| "macOS returned an invalid trash URL".into())
    })
}

/// Restore a specific item without replacing an existing destination.
pub(crate) fn restore_from_trash(
    trashed_path: &Path,
    original_path: &Path,
) -> Result<(), crate::curate::TrashRestoreError> {
    objc2::rc::autoreleasepool(|_| {
        if original_path
            .try_exists()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    crate::curate::TrashRestoreError::AccessDenied
                }
                _ => crate::curate::TrashRestoreError::OperationFailed,
            })?
        {
            return Err(crate::curate::TrashRestoreError::DestinationOccupied);
        }

        let source =
            file_url(trashed_path).map_err(|_| crate::curate::TrashRestoreError::InvalidReceipt)?;
        let destination = file_url(original_path)
            .map_err(|_| crate::curate::TrashRestoreError::InvalidReceipt)?;
        NSFileManager::defaultManager()
            .moveItemAtURL_toURL_error(&source, &destination)
            .map_err(|_| crate::curate::TrashRestoreError::OperationFailed)
    })
}

/// Ask the user which application should open `path`, then hand the original
/// file to that application through NSWorkspace.
///
/// `allowedFileTypes` and `openFile:withApplication:` are the synchronous
/// chooser APIs. The replacements require Uniform Type Identifiers or a
/// completion handler that would return before the user-mediated launch
/// finishes.
#[allow(deprecated)]
pub(crate) fn show_open_with_chooser(path: &Path) -> crate::open_with::OpenWithOutcome {
    use crate::open_with::OpenWithOutcome;

    let Ok(file) = file_url(path) else {
        return OpenWithOutcome::InvalidPath;
    };
    objc2::rc::autoreleasepool(|_| {
        let Some(mtm) = MainThreadMarker::new() else {
            return OpenWithOutcome::Failed;
        };
        let panel = NSOpenPanel::openPanel(mtm);
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setResolvesAliases(true);
        let title = NSString::from_str("Open With");
        panel.setTitle(Some(&title));
        let message = NSString::from_str("Choose the application that should open this image.");
        panel.setMessage(Some(&message));
        let prompt = NSString::from_str("Open");
        panel.setPrompt(Some(&prompt));
        if let Ok(applications) = file_url(Path::new("/Applications")) {
            panel.setDirectoryURL(Some(&applications));
        }
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str(
            "app",
        )])));
        if panel.runModal() != NSModalResponseOK {
            return OpenWithOutcome::Cancelled;
        }
        let Some(application) = panel.URL() else {
            return OpenWithOutcome::Cancelled;
        };
        let Some(file_path) = file_url_path(&file) else {
            return OpenWithOutcome::InvalidPath;
        };
        let Some(application_path) = file_url_path(&application) else {
            return OpenWithOutcome::Cancelled;
        };
        let Some(file_name) = path_as_nsstring(&file_path) else {
            return OpenWithOutcome::InvalidPath;
        };
        let Some(application_name) = path_as_nsstring(&application_path) else {
            return OpenWithOutcome::Cancelled;
        };
        if NSWorkspace::sharedWorkspace()
            .openFile_withApplication(&file_name, Some(&application_name))
        {
            OpenWithOutcome::Launched
        } else {
            OpenWithOutcome::Failed
        }
    })
}

fn path_as_nsstring(path: &Path) -> Option<Retained<NSString>> {
    Some(NSString::from_str(path.to_str()?))
}

fn file_url(path: &Path) -> Result<Retained<NSURL>, String> {
    let bytes = path.as_os_str().as_bytes();
    let representation =
        CString::new(bytes).map_err(|_| "macOS path contains an embedded null byte".to_owned())?;
    let pointer = NonNull::new(representation.as_ptr().cast_mut())
        .ok_or("macOS path representation is null")?;
    // SAFETY: `CString` supplies a valid, null-terminated filesystem path for
    // the duration of this call. NSURL copies the representation it needs.
    Ok(unsafe {
        NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(pointer, false, None)
    })
}

fn file_url_path(url: &NSURL) -> Option<PathBuf> {
    if !url.isFileURL() {
        return None;
    }

    let representation = url.fileSystemRepresentation();
    // SAFETY: Foundation documents `fileSystemRepresentation` as a stable,
    // null-terminated pointer for the lifetime of this autorelease context.
    let bytes = unsafe { CStr::from_ptr(representation.as_ptr()) }.to_bytes();
    (!bytes.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(test)]
mod tests {
    use super::{file_url, file_url_path, first_core_path};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use objc2_foundation::NSArray;

    #[test]
    fn file_url_round_trip_preserves_non_utf8_path_bytes() {
        let path = PathBuf::from(OsStr::from_bytes(b"/tmp/viewr_\xff.png"));
        let url = file_url(&path).unwrap();
        assert_eq!(file_url_path(&url), Some(path));
    }

    #[test]
    fn launch_services_selection_skips_non_core_files() {
        let text = file_url(Path::new("/tmp/readme.txt")).unwrap();
        let image = file_url(Path::new("/tmp/photo.png")).unwrap();
        let urls = NSArray::from_retained_slice(&[text, image]);
        assert_eq!(
            first_core_path(&urls),
            Some(PathBuf::from("/tmp/photo.png"))
        );
    }
}
