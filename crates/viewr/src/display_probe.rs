//! Thin platform fetch of display ICC bytes and Advanced Color facts.
#![allow(unsafe_code)] // DisplayConfig, GetICMProfile, and libX11 property reads
//!
//! Policy and admission live in `display_state`. This module only asks the
//! operating system for a file or a color-management flag. It never builds a
//! transform or writes pixels.

use crate::display_state::{DisplayHints, MonitorIdentity};

/// Refresh host color-management facts that can change when the window moves.
#[must_use]
pub(crate) fn refresh_display_hints(
    hints: DisplayHints,
    monitor: Option<&MonitorIdentity>,
) -> DisplayHints {
    #[cfg(windows)]
    {
        let mut hints = hints;
        hints.advanced_color = windows_advanced_color(monitor.and_then(MonitorIdentity::name));
        hints
    }
    #[cfg(not(windows))]
    {
        let _ = monitor;
        hints
    }
}

/// Fetch the display ICC associated with the monitor that owns the window.
///
/// Missing, oversized, unreadable, or non-file results become `None` so the
/// caller keeps the deterministic sRGB fallback. macOS and compositor-managed
/// Linux sessions do not fetch: those hosts already convert tagged sRGB.
#[must_use]
pub(crate) fn fetch_display_profile_bytes(
    monitor_name: Option<&str>,
    window: Option<&winit::window::Window>,
) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        let _ = window;
        windows_icm_profile_bytes(monitor_name)
    }
    #[cfg(target_os = "linux")]
    {
        let _ = monitor_name;
        x11_root_icc_profile(window)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (monitor_name, window);
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

/// Copy an X11 `_ICC_PROFILE` property only when it is a complete 8-bit blob.
#[cfg(any(target_os = "linux", test))]
#[must_use]
fn copy_x11_icc_property(
    format: i32,
    nitems: usize,
    bytes_after: usize,
    data: &[u8],
) -> Option<Vec<u8>> {
    if format != 8
        || bytes_after != 0
        || nitems == 0
        || nitems > viewr_protocol::MAX_COLOR_PROFILE_BYTES
    {
        return None;
    }
    (data.len() >= nitems).then(|| data[..nitems].to_vec())
}

#[cfg(windows)]
fn windows_icm_profile_bytes(monitor_name: Option<&str>) -> Option<Vec<u8>> {
    let path = windows_icm_profile_path(monitor_name)?;
    read_bounded_profile_file(&path)
}

/// Ask Windows whether Advanced Color / auto color management is enabled.
///
/// `None` means the query failed; policy then keeps compositor-managed sRGB so
/// a display ICC cannot be applied twice. Bit 1 of the Advanced Color info
/// value is `advancedColorEnabled`.
#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_advanced_color(monitor_name: Option<&str>) -> Option<bool> {
    use windows_sys::Win32::Devices::Display::{
        DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
        DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS,
        QueryDisplayConfig,
    };
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;

    let mut path_count = 0_u32;
    let mut mode_count = 0_u32;
    // Safety: both out-parameters are live stack integers.
    let sized = unsafe {
        GetDisplayConfigBufferSizes(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut path_count,
            &raw mut mode_count,
        )
    };
    if sized != ERROR_SUCCESS || path_count == 0 || path_count > 64 || mode_count > 128 {
        return None;
    }
    let mut paths =
        vec![unsafe { std::mem::zeroed::<DISPLAYCONFIG_PATH_INFO>() }; path_count as usize];
    let mut modes =
        vec![unsafe { std::mem::zeroed::<DISPLAYCONFIG_MODE_INFO>() }; mode_count as usize];
    // Safety: buffers match the counts just returned; topology id is optional
    // and may be null for QDC_ONLY_ACTIVE_PATHS.
    let queried = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut path_count,
            paths.as_mut_ptr(),
            &raw mut mode_count,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if queried != ERROR_SUCCESS {
        return None;
    }
    paths.truncate(path_count as usize);
    let wanted = monitor_name.and_then(wide_nul);
    let mut matched = None;
    for path in &paths {
        let mut source = unsafe { std::mem::zeroed::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() };
        source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        source.header.size = u32::try_from(std::mem::size_of_val(&source)).ok()?;
        source.header.adapterId = path.sourceInfo.adapterId;
        source.header.id = path.sourceInfo.id;
        // Safety: header size matches this stack struct; adapter and source
        // ids come from QueryDisplayConfig.
        let named = unsafe { DisplayConfigGetDeviceInfo(&raw mut source.header) };
        if named != 0 {
            continue;
        }
        let gdi_name = utf16_z(&source.viewGdiDeviceName);
        let is_match = wanted
            .as_ref()
            .is_none_or(|want| gdi_name.as_slice() == utf16_z(want));
        if !is_match {
            continue;
        }
        let mut info = unsafe { std::mem::zeroed::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() };
        info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
        info.header.size = u32::try_from(std::mem::size_of_val(&info)).ok()?;
        info.header.adapterId = path.targetInfo.adapterId;
        info.header.id = path.targetInfo.id;
        // Safety: header size matches this stack struct; adapter and target
        // ids come from QueryDisplayConfig.
        let got = unsafe { DisplayConfigGetDeviceInfo(&raw mut info.header) };
        if got != 0 {
            continue;
        }
        let enabled = (unsafe { info.Anonymous.value } & 0b10) != 0;
        if wanted.is_some() {
            return Some(enabled);
        }
        matched = Some(enabled);
    }
    matched
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

#[cfg(windows)]
fn utf16_z(units: &[u16]) -> Vec<u16> {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    units[..end].to_vec()
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn x11_root_icc_profile(window: Option<&winit::window::Window>) -> Option<Vec<u8>> {
    use std::ffi::CString;
    use std::os::raw::{c_int, c_long, c_uchar, c_ulong, c_void};

    use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

    let window = window?;
    let RawDisplayHandle::Xlib(handle) = window.display_handle().ok()?.as_raw() else {
        return None;
    };
    let display = handle.display?.as_ptr();
    let screen = handle.screen;

    let lib = CString::new("libX11.so.6").ok()?;
    // Safety: soname is a live C string. The handle is released before return.
    let x11 = unsafe { libc::dlopen(lib.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
    if x11.is_null() {
        return None;
    }

    type InternAtom = unsafe extern "C" fn(*mut c_void, *const i8, c_int) -> c_ulong;
    type RootWindow = unsafe extern "C" fn(*mut c_void, c_int) -> c_ulong;
    type GetWindowProperty = unsafe extern "C" fn(
        *mut c_void,
        c_ulong,
        c_ulong,
        c_long,
        c_long,
        c_int,
        c_ulong,
        *mut c_ulong,
        *mut c_int,
        *mut c_ulong,
        *mut c_ulong,
        *mut *mut c_uchar,
    ) -> c_int;
    type XFree = unsafe extern "C" fn(*mut c_void) -> c_int;

    let intern = unsafe { libc::dlsym(x11, c"XInternAtom".as_ptr()) };
    let root_window = unsafe { libc::dlsym(x11, c"XRootWindow".as_ptr()) };
    let get_prop = unsafe { libc::dlsym(x11, c"XGetWindowProperty".as_ptr()) };
    let x_free = unsafe { libc::dlsym(x11, c"XFree".as_ptr()) };
    if intern.is_null() || root_window.is_null() || get_prop.is_null() || x_free.is_null() {
        unsafe { libc::dlclose(x11) };
        return None;
    }
    let intern: InternAtom = unsafe { std::mem::transmute(intern) };
    let root_window: RootWindow = unsafe { std::mem::transmute(root_window) };
    let get_prop: GetWindowProperty = unsafe { std::mem::transmute(get_prop) };
    let x_free: XFree = unsafe { std::mem::transmute(x_free) };

    let names = [
        CString::new("_ICC_PROFILE").ok(),
        CString::new(format!("_ICC_PROFILE_{screen}")).ok(),
    ];
    let mut profile = None;
    for name in names.into_iter().flatten() {
        // Safety: display is the live Xlib connection winit owns. only_if_exists
        // is true so a missing atom is None rather than created.
        let atom = unsafe { intern(display, name.as_ptr(), 1) };
        if atom == 0 {
            continue;
        }
        let root = unsafe { root_window(display, screen) };
        if root == 0 {
            continue;
        }
        let max_longs = (viewr_protocol::MAX_COLOR_PROFILE_BYTES / 4) as c_long;
        let mut actual_type = 0_u64 as c_ulong;
        let mut actual_format = 0_i32;
        let mut nitems = 0_u64 as c_ulong;
        let mut bytes_after = 0_u64 as c_ulong;
        let mut data: *mut c_uchar = std::ptr::null_mut();
        // Safety: display and root belong to this window's X connection. The
        // property is read, not deleted. Out-parameters are live stack values.
        let status = unsafe {
            get_prop(
                display,
                root,
                atom,
                0,
                max_longs,
                0,
                0,
                &raw mut actual_type,
                &raw mut actual_format,
                &raw mut nitems,
                &raw mut bytes_after,
                &raw mut data,
            )
        };
        if status == 0 && !data.is_null() {
            let nitems = usize::try_from(nitems).unwrap_or(0);
            let slice = unsafe { std::slice::from_raw_parts(data, nitems) };
            profile = copy_x11_icc_property(
                actual_format,
                nitems,
                usize::try_from(bytes_after).unwrap_or(usize::MAX),
                slice,
            );
        }
        if !data.is_null() {
            unsafe { x_free(data.cast()) };
        }
        if profile.is_some() {
            break;
        }
    }
    unsafe { libc::dlclose(x11) };
    profile
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

    #[test]
    fn x11_icc_property_requires_complete_eight_bit_bytes() {
        let data = b"display icc bytes!!";
        assert_eq!(
            copy_x11_icc_property(8, data.len(), 0, data).as_deref(),
            Some(data.as_slice())
        );
        assert!(copy_x11_icc_property(16, data.len(), 0, data).is_none());
        assert!(copy_x11_icc_property(8, data.len(), 1, data).is_none());
        assert!(copy_x11_icc_property(8, 0, 0, data).is_none());
        assert!(copy_x11_icc_property(8, data.len() + 1, 0, data).is_none());
        assert!(
            copy_x11_icc_property(8, viewr_protocol::MAX_COLOR_PROFILE_BYTES + 1, 0, data)
                .is_none()
        );
    }
}
