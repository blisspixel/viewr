//! Launch prerequisites for the graphical session.
//!
//! A desktop viewer that cannot open a window must say so on stderr with a
//! non-zero exit rather than aborting inside a dynamic loader or exiting
//! quietly. Session detection, the required-library table, and every message
//! are pure so they can be tested without a display. Only the dynamic-library
//! probe touches the platform.

#[cfg(target_os = "linux")]
pub(crate) use unix_desktop::{
    DisplaySession, RequiredLibrary, detect_session, fallback_session, headless_message,
    missing_library_message, required_libraries, window_readiness_report,
};

/// Result of the doctor window-presentation section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowReadiness {
    /// Report lines, already prefixed and indented.
    pub(crate) lines: Vec<String>,
    /// `false` when this host is known to be unable to open a window.
    pub(crate) critical_ok: bool,
}

/// What doctor can honestly say about a GPU surface it never created.
const SURFACE_NOTE: &str =
    "[note] a GPU surface is proven only when viewr opens a window, not by doctor";

/// Launch-time message when the window exists but no GPU surface could be made.
pub(crate) fn gpu_failure_message(detail: &str, linux: bool) -> String {
    let mut message = format!(
        "cannot present images on this display: {detail}\n\
This usually means the session has no working GPU driver or software renderer.\n"
    );
    if linux {
        message.push_str(
            "Install working GPU drivers, or the Mesa software renderer:\n  \
Debian or Ubuntu: sudo apt install libgl1-mesa-dri\n  \
Fedora or RHEL: sudo dnf install mesa-dri-drivers\n  \
Arch: sudo pacman -S mesa\n",
        );
    } else {
        message.push_str("Update the graphics driver for this display and run viewr again.\n");
    }
    message
}

/// Doctor window-presentation section for platforms with a linked window system.
#[cfg(any(not(target_os = "linux"), test))]
pub(crate) fn native_window_readiness(platform: &str) -> WindowReadiness {
    WindowReadiness {
        lines: vec![
            format!("[ok]   window system: linked {platform} desktop libraries"),
            SURFACE_NOTE.to_owned(),
        ],
        critical_ok: true,
    }
}

/// Session variables that decide which windowing backend the process uses.
#[cfg(target_os = "linux")]
fn session_environment() -> (Option<String>, Option<String>, Option<String>) {
    (
        std::env::var("WINIT_UNIX_BACKEND").ok(),
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("DISPLAY").ok(),
    )
}

/// Current session as seen through the process environment.
#[cfg(target_os = "linux")]
pub(crate) fn current_session() -> DisplaySession {
    let (backend, wayland, x11) = session_environment();
    detect_session(backend.as_deref(), wayland.as_deref(), x11.as_deref())
}

/// The session viewr will actually present through, and the first library it
/// cannot load.
///
/// When the preferred backend is incomplete but the other one is fully
/// installed, winit starts on that one, so the report follows it rather than
/// blocking a launch that works.
#[cfg(target_os = "linux")]
pub(crate) fn resolve_window_support() -> (DisplaySession, Option<&'static RequiredLibrary>) {
    let session = current_session();
    let missing = first_missing_library(session);
    if missing.is_some() {
        let (backend, wayland, x11) = session_environment();
        let fallback = fallback_session(backend.as_deref(), wayland.as_deref(), x11.as_deref());
        if fallback != DisplaySession::None && first_missing_library(fallback).is_none() {
            return (fallback, None);
        }
    }
    (session, missing)
}

/// First required library the dynamic loader cannot resolve for this session.
#[cfg(target_os = "linux")]
pub(crate) fn first_missing_library(session: DisplaySession) -> Option<&'static RequiredLibrary> {
    required_libraries(session)
        .iter()
        .find(|library| !library.sonames.iter().copied().any(library_loads))
}

/// Ask the dynamic loader whether a soname resolves, using the same search the
/// windowing stack performs later.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // dlopen/dlclose is the only way to answer the loader question
fn library_loads(soname: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(soname) else {
        return false;
    };
    // SAFETY: `name` is a valid NUL-terminated C string that outlives the call,
    // and the returned handle is either null or released immediately below.
    unsafe {
        let handle = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return false;
        }
        libc::dlclose(handle);
    }
    true
}

/// Check that this process can reach a window before starting the event loop.
///
/// # Errors
/// Returns a complete, actionable message when no desktop session is reachable
/// or a windowing library the backend loads dynamically is not installed.
#[allow(
    clippy::unnecessary_wraps,
    reason = "platforms without a dynamic windowing preflight share one signature"
)]
pub(crate) fn preflight() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let (session, missing) = resolve_window_support();
        if session == DisplaySession::None {
            return Err(headless_message());
        }
        if let Some(library) = missing {
            return Err(missing_library_message(library));
        }
    }
    Ok(())
}

/// X11 and Wayland launch policy.
///
/// Compiled for Linux, and for tests on every platform, so these pure decisions
/// stay covered wherever the workspace is tested.
#[cfg(any(target_os = "linux", test))]
mod unix_desktop {
    use std::fmt::Write as _;

    use super::{SURFACE_NOTE, WindowReadiness};

    /// Which windowing backend the process will use on a Unix desktop.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum DisplaySession {
        /// `WAYLAND_DISPLAY` names a compositor.
        Wayland,
        /// `DISPLAY` names an X server.
        X11,
        /// No graphical session is reachable from this process.
        None,
    }

    impl DisplaySession {
        /// Short, path-free description used by doctor and launch failures.
        const fn summary(self) -> &'static str {
            match self {
                Self::Wayland => "Wayland (WAYLAND_DISPLAY is set)",
                Self::X11 => "X11 (DISPLAY is set)",
                Self::None => "none (WAYLAND_DISPLAY and DISPLAY are unset)",
            }
        }
    }

    /// A shared library the windowing stack loads at runtime rather than at link
    /// time, so a missing package is invisible to `ldd` until a window is created.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct RequiredLibrary {
        /// What the library is used for, in user words.
        pub(crate) purpose: &'static str,
        /// Accepted sonames, most specific first. Any one of them is sufficient.
        pub(crate) sonames: &'static [&'static str],
        /// Package name on Debian and Ubuntu.
        debian: &'static str,
        /// Package name on Fedora and RHEL.
        fedora: &'static str,
        /// Package name on Arch.
        arch: &'static str,
    }

    const XKBCOMMON: RequiredLibrary = RequiredLibrary {
        purpose: "keyboard layout handling",
        sonames: &["libxkbcommon.so.0", "libxkbcommon.so"],
        debian: "libxkbcommon0",
        fedora: "libxkbcommon",
        arch: "libxkbcommon",
    };

    const XKBCOMMON_X11: RequiredLibrary = RequiredLibrary {
        purpose: "X11 keyboard layout handling",
        sonames: &["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"],
        debian: "libxkbcommon-x11-0",
        fedora: "libxkbcommon-x11",
        arch: "libxkbcommon-x11",
    };

    const XLIB: RequiredLibrary = RequiredLibrary {
        purpose: "X11 window creation",
        sonames: &["libX11.so.6", "libX11.so"],
        debian: "libx11-6",
        fedora: "libX11",
        arch: "libx11",
    };

    const WAYLAND_CLIENT: RequiredLibrary = RequiredLibrary {
        purpose: "Wayland window creation",
        sonames: &["libwayland-client.so.0", "libwayland-client.so"],
        debian: "libwayland-client0",
        fedora: "libwayland-client",
        arch: "wayland",
    };

    const X11_LIBRARIES: &[RequiredLibrary] = &[XKBCOMMON, XKBCOMMON_X11, XLIB];
    const WAYLAND_LIBRARIES: &[RequiredLibrary] = &[XKBCOMMON, WAYLAND_CLIENT];

    /// Resolve the session the windowing stack will use.
    ///
    /// `WINIT_UNIX_BACKEND` overrides automatic selection, matching winit.
    pub(crate) fn detect_session(
        backend_override: Option<&str>,
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
    ) -> DisplaySession {
        let wayland = wayland_display.is_some_and(|value| !value.is_empty());
        let x11 = x11_display.is_some_and(|value| !value.is_empty());
        match backend_override.map(str::trim) {
            Some("x11") if x11 => DisplaySession::X11,
            Some("wayland") if wayland => DisplaySession::Wayland,
            Some("x11" | "wayland") => DisplaySession::None,
            _ if wayland => DisplaySession::Wayland,
            _ if x11 => DisplaySession::X11,
            _ => DisplaySession::None,
        }
    }

    /// The backend winit tries when the preferred one cannot start.
    ///
    /// An explicit `WINIT_UNIX_BACKEND` choice has no fallback, and a compositor
    /// is never a fallback for X11 because winit prefers Wayland already.
    pub(crate) fn fallback_session(
        backend_override: Option<&str>,
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
    ) -> DisplaySession {
        if backend_override.is_some_and(|value| matches!(value.trim(), "x11" | "wayland")) {
            return DisplaySession::None;
        }
        match detect_session(None, wayland_display, x11_display) {
            DisplaySession::Wayland => detect_session(Some("x11"), None, x11_display),
            DisplaySession::X11 | DisplaySession::None => DisplaySession::None,
        }
    }

    /// Libraries the given session loads dynamically during window creation.
    pub(crate) const fn required_libraries(session: DisplaySession) -> &'static [RequiredLibrary] {
        match session {
            DisplaySession::Wayland => WAYLAND_LIBRARIES,
            DisplaySession::X11 => X11_LIBRARIES,
            // Without a session there is no backend to load, and CLI
            // subcommands must keep working on headless hosts.
            DisplaySession::None => &[],
        }
    }

    /// Package-manager guidance for one missing library, indented for a report.
    fn install_hint(library: &RequiredLibrary, indent: &str) -> String {
        let mut hint = String::new();
        for (distribution, command, package) in [
            ("Debian or Ubuntu", "sudo apt install", library.debian),
            ("Fedora or RHEL", "sudo dnf install", library.fedora),
            ("Arch", "sudo pacman -S", library.arch),
        ] {
            let _ = writeln!(hint, "{indent}{distribution}: {command} {package}");
        }
        hint
    }

    /// Launch-time message for a windowing library the loader cannot find.
    pub(crate) fn missing_library_message(library: &RequiredLibrary) -> String {
        format!(
            "cannot open a window: {soname} is not installed, and this session needs it for \
{purpose}.\nInstall it with your package manager, then run viewr again:\n{hint}",
            soname = library.sonames[0],
            purpose = library.purpose,
            hint = install_hint(library, "  "),
        )
    }

    /// Launch-time message when no desktop session is reachable.
    pub(crate) fn headless_message() -> String {
        "cannot open a window: no graphical session was found (WAYLAND_DISPLAY and DISPLAY are \
unset).\nviewr is a desktop viewer and needs a running desktop session. `viewr doctor`, \
`viewr benchmark`, and `viewr version` still work from a terminal."
            .to_owned()
    }

    /// Build the doctor window-presentation section for a Unix desktop.
    pub(crate) fn window_readiness_report(
        session: DisplaySession,
        missing: Option<&RequiredLibrary>,
    ) -> WindowReadiness {
        let mut lines = Vec::new();
        let mut critical_ok = true;
        match session {
            DisplaySession::None => {
                lines.push(format!("[WARN] display session: {}", session.summary()));
                lines.push(
                    "       viewr cannot open a window here; CLI subcommands still work".to_owned(),
                );
            }
            DisplaySession::Wayland | DisplaySession::X11 => {
                lines.push(format!("[ok]   display session: {}", session.summary()));
                if let Some(library) = missing {
                    critical_ok = false;
                    lines.push(format!(
                        "[FAIL] windowing library: {} is missing ({})",
                        library.sonames[0], library.purpose
                    ));
                    lines.extend(install_hint(library, "       ").lines().map(str::to_owned));
                } else {
                    let present: Vec<&str> = required_libraries(session)
                        .iter()
                        .map(|library| library.sonames[0])
                        .collect();
                    lines.push(format!(
                        "[ok]   windowing libraries: {} present",
                        present.join(", ")
                    ));
                }
            }
        }
        lines.push(SURFACE_NOTE.to_owned());
        WindowReadiness { lines, critical_ok }
    }

    #[cfg(test)]
    mod tests {
        use super::{
            DisplaySession, WAYLAND_CLIENT, XKBCOMMON_X11, detect_session, fallback_session,
            headless_message, install_hint, missing_library_message, required_libraries,
            window_readiness_report,
        };

        #[test]
        fn session_detection_prefers_wayland_and_honors_the_backend_override() {
            assert_eq!(
                detect_session(None, Some("wayland-0"), Some(":0")),
                DisplaySession::Wayland
            );
            assert_eq!(detect_session(None, None, Some(":6")), DisplaySession::X11);
            assert_eq!(
                detect_session(None, Some(""), Some("")),
                DisplaySession::None
            );
            assert_eq!(detect_session(None, None, None), DisplaySession::None);
            assert_eq!(
                detect_session(Some("x11"), Some("wayland-0"), Some(":0")),
                DisplaySession::X11
            );
            assert_eq!(
                detect_session(Some("wayland"), None, Some(":0")),
                DisplaySession::None
            );
            assert_eq!(
                detect_session(Some("x11"), Some("wayland-0"), None),
                DisplaySession::None
            );
            assert_eq!(
                detect_session(Some("other"), None, Some(":0")),
                DisplaySession::X11
            );
        }

        #[test]
        fn an_incomplete_wayland_session_can_still_fall_back_to_x11() {
            assert_eq!(
                fallback_session(None, Some("wayland-0"), Some(":0")),
                DisplaySession::X11
            );
            // Nothing to fall back to.
            assert_eq!(
                fallback_session(None, Some("wayland-0"), None),
                DisplaySession::None
            );
            assert_eq!(
                fallback_session(None, None, Some(":0")),
                DisplaySession::None
            );
            assert_eq!(fallback_session(None, None, None), DisplaySession::None);
            // An explicit backend choice is not second-guessed.
            assert_eq!(
                fallback_session(Some("wayland"), Some("wayland-0"), Some(":0")),
                DisplaySession::None
            );
            assert_eq!(
                fallback_session(Some("x11"), Some("wayland-0"), Some(":0")),
                DisplaySession::None
            );
        }

        #[test]
        fn each_session_declares_the_libraries_its_backend_loads() {
            let names = |session| {
                required_libraries(session)
                    .iter()
                    .map(|library| library.sonames[0])
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                names(DisplaySession::X11),
                ["libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libX11.so.6"]
            );
            assert_eq!(
                names(DisplaySession::Wayland),
                ["libxkbcommon.so.0", "libwayland-client.so.0"]
            );
            assert!(required_libraries(DisplaySession::None).is_empty());
        }

        #[test]
        fn a_missing_library_names_the_package_for_each_supported_distribution() {
            let message = missing_library_message(&XKBCOMMON_X11);
            assert!(message.starts_with(
                "cannot open a window: libxkbcommon-x11.so.0 is not installed, and this session \
needs it for X11 keyboard layout handling."
            ));
            assert!(message.contains("sudo apt install libxkbcommon-x11-0"));
            assert!(message.contains("sudo dnf install libxkbcommon-x11"));
            assert!(message.contains("sudo pacman -S libxkbcommon-x11"));

            let indented = install_hint(&WAYLAND_CLIENT, "       ");
            assert!(indented.lines().all(|line| line.starts_with("       ")));
            assert!(indented.contains("sudo apt install libwayland-client0"));
        }

        #[test]
        fn a_headless_host_is_told_what_still_works() {
            let message = headless_message();
            assert!(message.contains("no graphical session was found"));
            assert!(message.contains("viewr doctor"));
        }

        #[test]
        fn doctor_window_section_fails_only_when_a_session_cannot_load_its_libraries() {
            let ready = window_readiness_report(DisplaySession::X11, None);
            assert!(ready.critical_ok);
            assert!(ready.lines[0].contains("display session: X11"));
            assert!(ready.lines[1].starts_with("[ok]   windowing libraries"));
            assert!(ready.lines[1].contains("libxkbcommon-x11.so.0"));

            let broken = window_readiness_report(DisplaySession::X11, Some(&XKBCOMMON_X11));
            assert!(!broken.critical_ok);
            assert!(
                broken
                    .lines
                    .iter()
                    .any(|line| line.starts_with("[FAIL] windowing library: libxkbcommon-x11.so.0"))
            );
            assert!(
                broken
                    .lines
                    .iter()
                    .any(|line| line.contains("sudo apt install libxkbcommon-x11-0"))
            );

            let headless = window_readiness_report(DisplaySession::None, None);
            assert!(headless.critical_ok);
            assert!(headless.lines[0].starts_with("[WARN] display session: none"));
            assert!(headless.lines[1].contains("CLI subcommands still work"));

            for report in [
                window_readiness_report(DisplaySession::Wayland, None),
                window_readiness_report(DisplaySession::None, None),
            ] {
                assert!(
                    report
                        .lines
                        .last()
                        .is_some_and(|line| line.contains("proven only when viewr opens a window"))
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{gpu_failure_message, native_window_readiness};

    #[test]
    fn a_failed_surface_reports_itself_without_developer_logging() {
        let linux = gpu_failure_message("create_surface: no enabled backend", true);
        assert!(linux.starts_with("cannot present images on this display: create_surface:"));
        assert!(linux.contains("no working GPU driver or software renderer"));
        assert!(linux.contains("sudo apt install libgl1-mesa-dri"));

        let other = gpu_failure_message("device request failed", false);
        assert!(other.contains("device request failed"));
        assert!(other.contains("Update the graphics driver"));
        assert!(!other.contains("apt"));
    }

    /// The loader probe and the launch decision, exercised on the real host.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_launch_check_asks_the_loader_and_agrees_with_its_own_report() {
        use super::{
            DisplaySession, current_session, library_loads, preflight, resolve_window_support,
        };

        assert!(library_loads("libc.so.6"));
        assert!(!library_loads("libviewr-does-not-exist.so.0"));
        assert!(!library_loads("interior\0nul.so"));

        let (resolved, missing) = resolve_window_support();
        // A fallback is reported only when it resolves every library it needs.
        if missing.is_some() {
            assert_eq!(resolved, current_session());
        }
        assert_eq!(
            preflight().is_ok(),
            resolved != DisplaySession::None && missing.is_none()
        );
    }

    #[test]
    fn a_linked_window_system_still_refuses_to_claim_a_surface() {
        let readiness = native_window_readiness("Windows");
        assert!(readiness.critical_ok);
        assert_eq!(
            readiness.lines,
            [
                "[ok]   window system: linked Windows desktop libraries",
                "[note] a GPU surface is proven only when viewr opens a window, not by doctor",
            ]
        );
    }
}
