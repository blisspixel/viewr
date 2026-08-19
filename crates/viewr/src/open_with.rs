//! User-mediated Open With choosers.
//!
//! Windows uses `SHOpenWithDialog`. macOS uses an application picker plus
//! `NSWorkspace`. Linux uses the desktop-portal `OpenURI` chooser with `ask`.
//! None of these paths constructs a shell command or silently launches a
//! default application.
#![allow(unsafe_code)]

use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenWithOutcome {
    Launched,
    Cancelled,
    InvalidPath,
    Failed,
}

#[cfg(target_os = "windows")]
pub(crate) fn show_open_with_dialog(
    path: &Path,
    parent: windows_sys::Win32::Foundation::HWND,
) -> OpenWithOutcome {
    show_windows_open_with_dialog(path, parent)
}

#[cfg(target_os = "macos")]
pub(crate) fn show_open_with_dialog(path: &Path) -> OpenWithOutcome {
    crate::macos::show_open_with_chooser(path)
}

#[cfg(target_os = "linux")]
pub(crate) fn show_open_with_dialog(path: &Path) -> OpenWithOutcome {
    show_linux_open_with_dialog(path)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub(crate) fn show_open_with_dialog(_path: &Path) -> OpenWithOutcome {
    OpenWithOutcome::Failed
}

#[cfg(target_os = "windows")]
fn classify_open_with_hresult(result: i32) -> OpenWithOutcome {
    const HRESULT_CANCELLED: u32 = 0x8007_04c7;
    match result {
        0 => OpenWithOutcome::Launched,
        value if value.cast_unsigned() == HRESULT_CANCELLED => OpenWithOutcome::Cancelled,
        _ => OpenWithOutcome::Failed,
    }
}

#[cfg(target_os = "windows")]
fn show_windows_open_with_dialog(
    path: &Path,
    parent: windows_sys::Win32::Foundation::HWND,
) -> OpenWithOutcome {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{OAIF_EXEC, OPENASINFO, SHOpenWithDialog};

    let mut path_wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if path_wide.contains(&0) {
        return OpenWithOutcome::InvalidPath;
    }
    path_wide.push(0);
    let request = OPENASINFO {
        pcszFile: path_wide.as_ptr(),
        pcszClass: std::ptr::null(),
        oaifInFlags: OAIF_EXEC,
    };
    // SAFETY: `path_wide` remains alive and NUL-terminated for the synchronous
    // call, `request` contains valid pointers, and `parent` is either viewr's
    // live HWND or null as explicitly accepted by the Windows API.
    classify_open_with_hresult(unsafe { SHOpenWithDialog(parent, &raw const request) })
}

#[cfg(target_os = "linux")]
const PORTAL_DESKTOP_DEST: &str = "org.freedesktop.portal.Desktop";
#[cfg(target_os = "linux")]
const PORTAL_DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
#[cfg(target_os = "linux")]
const PORTAL_OPEN_URI_INTERFACE: &str = "org.freedesktop.portal.OpenURI";
/// `OpenURI` takes a URI string. `OpenFile` takes a file descriptor and must
/// not be called with a `file://` path.
#[cfg(target_os = "linux")]
const PORTAL_OPEN_URI_METHOD: &str = "OpenURI";

#[cfg(target_os = "linux")]
fn show_linux_open_with_dialog(path: &Path) -> OpenWithOutcome {
    linux_open_uri(path).unwrap_or(OpenWithOutcome::Failed)
}

#[cfg(target_os = "linux")]
fn linux_open_uri(path: &Path) -> Option<OpenWithOutcome> {
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

    let uri = file_uri(path)?;
    let connection = Connection::session().ok()?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        PORTAL_DESKTOP_DEST,
        PORTAL_DESKTOP_PATH,
        PORTAL_OPEN_URI_INTERFACE,
    )
    .ok()?;
    let mut options = HashMap::new();
    options.insert("ask", Value::from(true));
    let handle: OwnedObjectPath = proxy
        .call(PORTAL_OPEN_URI_METHOD, &("", uri.as_str(), options))
        .ok()?;
    let handle = ObjectPath::try_from(handle.as_str()).ok()?;
    let request = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        handle.as_str(),
        "org.freedesktop.portal.Request",
    )
    .ok()?;
    let signal = request.receive_signal("Response").ok()?;
    let message = signal.into_iter().next()?;
    let (code, _results): (u32, HashMap<String, Value<'_>>) = message.body().deserialize().ok()?;
    Some(match code {
        0 => OpenWithOutcome::Launched,
        1 => OpenWithOutcome::Cancelled,
        _ => OpenWithOutcome::Failed,
    })
}

#[cfg(target_os = "linux")]
fn file_uri(path: &Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;

    let path = path.canonicalize().ok()?;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                uri.push(char::from(byte));
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    Some(uri)
}

#[cfg(test)]
mod tests {
    use super::OpenWithOutcome;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_open_with_hresult_is_classified_without_path_details() {
        assert_eq!(
            super::classify_open_with_hresult(0),
            OpenWithOutcome::Launched
        );
        assert_eq!(
            super::classify_open_with_hresult(0x8007_04c7_u32.cast_signed()),
            OpenWithOutcome::Cancelled
        );
        assert_eq!(
            super::classify_open_with_hresult(0x8000_4005_u32.cast_signed()),
            OpenWithOutcome::Failed
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_open_with_uses_open_uri_with_ask_not_open_file() {
        assert_eq!(super::PORTAL_OPEN_URI_METHOD, "OpenURI");
        assert_eq!(
            super::PORTAL_OPEN_URI_INTERFACE,
            "org.freedesktop.portal.OpenURI"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_file_uri_percent_encodes_only_untrusted_bytes() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("hello world.png");
        std::fs::write(&path, b"fixture").expect("write spaced name");
        let uri = super::file_uri(&path).expect("canonicalize spaced path");
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("hello%20world.png"));
        assert!(!uri.contains(' '));
    }

    #[test]
    fn failed_and_cancelled_are_distinct_from_a_launch() {
        assert_ne!(OpenWithOutcome::Failed, OpenWithOutcome::Launched);
        assert_ne!(OpenWithOutcome::Cancelled, OpenWithOutcome::Launched);
        assert_ne!(OpenWithOutcome::InvalidPath, OpenWithOutcome::Launched);
    }
}
