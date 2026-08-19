//! Thin platform fetch of display ICC bytes.
//!
//! Policy and admission live in `display_state`. This module only asks the
//! operating system for a file and reads it under the protocol size cap. It
//! never builds a transform or writes pixels.

/// Fetch the display ICC associated with the monitor that owns the window.
///
/// Missing, oversized, unreadable, or non-file results become `None` so the
/// caller keeps the deterministic sRGB fallback. macOS and compositor-managed
/// Linux sessions do not fetch: those hosts already convert tagged sRGB.
#[must_use]
pub(crate) fn fetch_display_profile_bytes(monitor_name: Option<&str>) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        windows_icm_profile_bytes(monitor_name)
    }
    #[cfg(not(windows))]
    {
        let _ = monitor_name;
        None
    }
}

/// Read one regular file if it is within the protocol ICC bound.
#[cfg(any(windows, test))]
#[must_use]
pub(crate) fn read_bounded_profile_file(path: &std::path::Path) -> Option<Vec<u8>> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let length = usize::try_from(metadata.len()).ok()?;
    if length == 0 || length > viewr_protocol::MAX_COLOR_PROFILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    (bytes.len() == length && bytes.len() <= viewr_protocol::MAX_COLOR_PROFILE_BYTES)
        .then_some(bytes)
}

#[cfg(windows)]
fn windows_icm_profile_bytes(monitor_name: Option<&str>) -> Option<Vec<u8>> {
    let path = windows_icm_profile_path(monitor_name)?;
    read_bounded_profile_file(&path)
}

/// Ask Windows for the ICM profile path of one display device.
///
/// `GetICMProfileW` writes a filesystem path, not profile bytes. The path is
/// treated as untrusted operating-system output and only then read under the
/// same size cap as an embedded ICC.
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_icm_profile_path(monitor_name: Option<&str>) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Graphics::Gdi::{CreateDCW, DeleteDC};
    use windows_sys::Win32::UI::ColorSystem::GetICMProfileW;

    let driver = wide_nul("DISPLAY")?;
    let device = match monitor_name {
        Some(name) => Some(wide_nul(name)?),
        None => None,
    };
    let device_ptr = device.as_ref().map_or(std::ptr::null(), Vec::as_ptr);

    // Safety: driver is a NUL-terminated "DISPLAY" string. device_ptr is
    // either null (the default display) or a NUL-terminated monitor name.
    // The DEVMODE pointer is null, which CreateDCW documents as the default
    // mode. The returned DC is exclusively owned and released below.
    let dc = unsafe {
        CreateDCW(
            driver.as_ptr(),
            device_ptr,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if dc.is_null() {
        return None;
    }

    let mut characters: u32 = 0;
    // Safety: size-only query. The buffer pointer is null and the length
    // out-parameter is a live u32. A false return still leaves `characters`
    // as the required count when Windows reports one.
    let sized = unsafe { GetICMProfileW(dc, &raw mut characters, std::ptr::null_mut()) };
    if characters == 0 || characters > 32_768 {
        // Safety: exclusive DC from CreateDCW above.
        unsafe { DeleteDC(dc) };
        return None;
    }
    let mut buffer = vec![0u16; characters as usize];
    // Safety: buffer length matches the character count Windows just
    // reported. The DC is still the one CreateDCW returned.
    let got = unsafe { GetICMProfileW(dc, &raw mut characters, buffer.as_mut_ptr()) } != 0;
    // Safety: exclusive DC from CreateDCW above.
    unsafe { DeleteDC(dc) };
    if !got {
        return None;
    }
    let _ = sized;
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    if end == 0 {
        return None;
    }
    let path = std::ffi::OsString::from_wide(&buffer[..end]);
    let path = std::path::PathBuf::from(path);
    path.is_absolute().then_some(path)
}

#[cfg(windows)]
fn wide_nul(value: &str) -> Option<Vec<u16>> {
    if value.contains('\0') {
        return None;
    }
    Some(value.encode_utf16().chain(std::iter::once(0)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_profile_read_rejects_missing_empty_and_oversized_files() {
        let directory = tempfile::tempdir().expect("temp profile directory");
        let missing = directory.path().join("missing.icc");
        assert!(read_bounded_profile_file(&missing).is_none());

        let empty = directory.path().join("empty.icc");
        std::fs::write(&empty, []).expect("write empty profile");
        assert!(read_bounded_profile_file(&empty).is_none());

        let oversized = directory.path().join("oversized.icc");
        std::fs::write(
            &oversized,
            vec![0; viewr_protocol::MAX_COLOR_PROFILE_BYTES + 1],
        )
        .expect("write oversized profile");
        assert!(read_bounded_profile_file(&oversized).is_none());
    }

    #[test]
    fn bounded_profile_read_returns_an_exact_regular_file() {
        let directory = tempfile::tempdir().expect("temp profile directory");
        let path = directory.path().join("display.icc");
        let expected = b"not a real ICC, but bounded";
        std::fs::write(&path, expected).expect("write profile fixture");
        assert_eq!(
            read_bounded_profile_file(&path).as_deref(),
            Some(expected.as_slice())
        );
    }
}
