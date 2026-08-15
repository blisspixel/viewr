//! Launch prerequisites for the graphical session.
//!
//! A desktop viewer that cannot open a window must say so on stderr with a
//! non-zero exit rather than aborting inside a dynamic loader or exiting
//! quietly. Session detection, the required-library tables, backend selection,
//! and every message are pure so they can be tested without a display. Only the
//! dynamic-library probe and the environment reads touch the platform.

#[cfg(target_os = "linux")]
pub(crate) use unix_desktop::{
    DisplaySession, RequiredLibrary, WindowSupport, compositor_missing_message, detect_session,
    event_loop_failure_message, fallback_session, gpu_advice, gpu_runtime_libraries,
    headless_message, missing_library_message, required_libraries, wayland_socket_path,
    window_readiness_report,
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

/// Advice for a failed surface on a platform whose graphics stack is linked.
#[cfg(any(not(target_os = "linux"), test))]
const NATIVE_GPU_ADVICE: &str =
    "Update the graphics driver for this display and run viewr again.\n";

/// Launch-time message when the window exists but no GPU surface could be made.
pub(crate) fn gpu_failure_message(detail: &str, advice: &str) -> String {
    format!("cannot present images on this display: {detail}\n{advice}")
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
fn session_environment() -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    (
        std::env::var("WINIT_UNIX_BACKEND").ok(),
        std::env::var("WAYLAND_DISPLAY").ok(),
        std::env::var("DISPLAY").ok(),
        std::env::var("XDG_RUNTIME_DIR").ok(),
    )
}

/// Whether `WAYLAND_DISPLAY` names a compositor socket that exists.
#[cfg(target_os = "linux")]
fn wayland_compositor_reachable(wayland_display: Option<&str>, runtime_dir: Option<&str>) -> bool {
    wayland_socket_path(wayland_display, runtime_dir)
        .is_some_and(|socket| std::path::Path::new(&socket).exists())
}

/// The backend viewr will present through, and what that backend is missing.
///
/// A Wayland variable whose compositor is not running resolves to X11 when an X
/// server is available, and a backend whose libraries are incomplete falls back
/// to the other one when that is complete. Launch and doctor share this result
/// so they never disagree about which backend runs.
#[cfg(target_os = "linux")]
pub(crate) fn resolve_window_support() -> WindowSupport {
    let (backend, wayland, x11, runtime_dir) = session_environment();
    let reachable = wayland_compositor_reachable(wayland.as_deref(), runtime_dir.as_deref());
    let compositor_unreachable =
        wayland.as_deref().is_some_and(|value| !value.is_empty()) && !reachable;
    let session = detect_session(
        backend.as_deref(),
        wayland.as_deref(),
        x11.as_deref(),
        reachable,
    );
    let mut missing_library = first_missing_library(session);
    let mut session = session;
    if missing_library.is_some() {
        let fallback = fallback_session(backend.as_deref(), wayland.as_deref(), x11.as_deref());
        if fallback != DisplaySession::None && first_missing_library(fallback).is_none() {
            session = fallback;
            missing_library = None;
        }
    }
    let (egl, vulkan) = gpu_runtime_present();
    WindowSupport {
        session,
        missing_library,
        compositor_unreachable,
        egl,
        vulkan,
    }
}

/// First required library the dynamic loader cannot resolve for this session.
#[cfg(target_os = "linux")]
pub(crate) fn first_missing_library(session: DisplaySession) -> Option<&'static RequiredLibrary> {
    required_libraries(session)
        .iter()
        .find(|library| !library.sonames.iter().copied().any(library_loads))
}

/// Whether the EGL and Vulkan runtimes wgpu can use are installed.
#[cfg(target_os = "linux")]
pub(crate) fn gpu_runtime_present() -> (bool, bool) {
    let loadable = |library: &RequiredLibrary| library.sonames.iter().copied().any(library_loads);
    let runtimes = gpu_runtime_libraries();
    (loadable(&runtimes[0]), loadable(&runtimes[1]))
}

/// Ask the dynamic loader whether a soname resolves, using the same search the
/// windowing and graphics stacks perform later.
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

/// The backend the event loop should be pinned to, when viewr chose one.
///
/// `None` leaves winit's own selection alone, which is correct when the user
/// set `WINIT_UNIX_BACKEND` or when no session is reachable.
#[cfg(target_os = "linux")]
pub(crate) fn preferred_backend() -> Option<DisplaySession> {
    let (backend, ..) = session_environment();
    if backend.is_some_and(|value| matches!(value.trim(), "x11" | "wayland")) {
        return None;
    }
    match resolve_window_support().session {
        DisplaySession::None => None,
        session => Some(session),
    }
}

/// Complete stderr message for a GPU surface failure on this host.
pub(crate) fn host_gpu_failure_message(detail: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        let (egl, vulkan) = gpu_runtime_present();
        gpu_failure_message(detail, &gpu_advice(egl, vulkan))
    }
    #[cfg(not(target_os = "linux"))]
    {
        gpu_failure_message(detail, NATIVE_GPU_ADVICE)
    }
}

/// Complete stderr message for an event loop that could not start.
pub(crate) fn host_event_loop_failure_message(detail: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        event_loop_failure_message(detail, resolve_window_support().compositor_unreachable)
    }
    #[cfg(not(target_os = "linux"))]
    {
        format!("cannot open a window: {detail}")
    }
}

/// Window-presentation readiness for the current platform.
pub(crate) fn host_window_readiness() -> WindowReadiness {
    #[cfg(target_os = "linux")]
    {
        let support = resolve_window_support();
        window_readiness_report(&support)
    }
    #[cfg(not(target_os = "linux"))]
    {
        native_window_readiness(match std::env::consts::OS {
            "windows" => "Windows",
            "macos" => "macOS",
            other => other,
        })
    }
}

/// Check that this process can reach a window before starting the event loop.
///
/// # Errors
/// Returns a complete, actionable message when no desktop session is reachable,
/// a windowing library the backend loads dynamically is not installed, or no
/// graphics runtime exists for wgpu to create a surface with.
#[allow(
    clippy::unnecessary_wraps,
    reason = "platforms without a dynamic windowing preflight share one signature"
)]
pub(crate) fn preflight() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let support = resolve_window_support();
        if support.session == DisplaySession::None {
            return Err(if support.compositor_unreachable {
                compositor_missing_message()
            } else {
                headless_message()
            });
        }
        if let Some(library) = support.missing_library {
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

    /// Everything the launch and doctor paths need to agree on for this host.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct WindowSupport {
        /// The backend viewr will actually use.
        pub(crate) session: DisplaySession,
        /// First windowing library the loader cannot resolve for that backend.
        pub(crate) missing_library: Option<&'static RequiredLibrary>,
        /// `WAYLAND_DISPLAY` names a compositor socket that does not exist.
        pub(crate) compositor_unreachable: bool,
        /// `libEGL` is installed, so wgpu's GL backend can initialize.
        pub(crate) egl: bool,
        /// `libvulkan` is installed, so wgpu's Vulkan backend can initialize.
        pub(crate) vulkan: bool,
    }

    /// Which windowing backend the process will use on a Unix desktop.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum DisplaySession {
        /// `WAYLAND_DISPLAY` names a compositor that is running.
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

    /// A shared library the windowing or graphics stack loads at runtime rather
    /// than at link time, so a missing package is invisible to `ldd` until a
    /// window or a GPU surface is created.
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

    /// wgpu's GL backend loads EGL at run time; without it that backend cannot
    /// initialize, even when Mesa's DRI drivers are installed.
    const EGL: RequiredLibrary = RequiredLibrary {
        purpose: "the OpenGL backend, including Mesa software rendering",
        sonames: &["libEGL.so.1", "libEGL.so"],
        debian: "libegl1 libegl-mesa0",
        fedora: "mesa-libEGL",
        arch: "mesa",
    };

    /// wgpu's Vulkan backend loads the loader at run time and still needs an
    /// installed driver behind it.
    const VULKAN: RequiredLibrary = RequiredLibrary {
        purpose: "the Vulkan backend",
        sonames: &["libvulkan.so.1", "libvulkan.so"],
        debian: "mesa-vulkan-drivers",
        fedora: "mesa-vulkan-drivers",
        arch: "vulkan-swrast",
    };

    const X11_LIBRARIES: &[RequiredLibrary] = &[XKBCOMMON, XKBCOMMON_X11, XLIB];
    const WAYLAND_LIBRARIES: &[RequiredLibrary] = &[XKBCOMMON, WAYLAND_CLIENT];
    const GPU_RUNTIME_LIBRARIES: &[RequiredLibrary] = &[EGL, VULKAN];

    /// The graphics runtimes wgpu can initialize on this platform.
    pub(crate) const fn gpu_runtime_libraries() -> &'static [RequiredLibrary] {
        GPU_RUNTIME_LIBRARIES
    }

    /// Resolve the socket `WAYLAND_DISPLAY` names, per the Wayland protocol.
    ///
    /// An absolute value is the socket itself. A bare name is relative to
    /// `XDG_RUNTIME_DIR`, and without that directory there is nothing to reach.
    pub(crate) fn wayland_socket_path(
        wayland_display: Option<&str>,
        runtime_dir: Option<&str>,
    ) -> Option<String> {
        let display = wayland_display
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        if display.starts_with('/') {
            return Some(display.to_owned());
        }
        let directory = runtime_dir
            .map(str::trim)
            .filter(|value| !value.is_empty())?
            .trim_end_matches('/');
        Some(format!("{directory}/{display}"))
    }

    /// Resolve the session the windowing stack will use.
    ///
    /// `WINIT_UNIX_BACKEND` overrides automatic selection, matching winit. A
    /// Wayland variable whose compositor is not running is not a session, so an
    /// available X server wins instead of failing the launch.
    pub(crate) fn detect_session(
        backend_override: Option<&str>,
        wayland_display: Option<&str>,
        x11_display: Option<&str>,
        wayland_reachable: bool,
    ) -> DisplaySession {
        let wayland = wayland_display.is_some_and(|value| !value.is_empty());
        let x11 = x11_display.is_some_and(|value| !value.is_empty());
        match backend_override.map(str::trim) {
            Some("x11") if x11 => DisplaySession::X11,
            Some("wayland") if wayland => DisplaySession::Wayland,
            Some("x11" | "wayland") => DisplaySession::None,
            _ if wayland && wayland_reachable => DisplaySession::Wayland,
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
        match detect_session(None, wayland_display, x11_display, true) {
            DisplaySession::Wayland => detect_session(Some("x11"), None, x11_display, false),
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

    /// Package-manager guidance for one library, indented for a report.
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

    /// Launch-time message when `WAYLAND_DISPLAY` points at nothing.
    pub(crate) fn compositor_missing_message() -> String {
        "cannot open a window: WAYLAND_DISPLAY names a compositor socket that does not exist, and \
DISPLAY is unset.\nStart a desktop session, or unset WAYLAND_DISPLAY when the session is X11. \
`viewr doctor`, `viewr benchmark`, and `viewr version` still work from a terminal."
            .to_owned()
    }

    /// Keep a platform error's sentence and drop build-machine paths from it.
    ///
    /// Dependency errors embed the source file they were raised in, which is a
    /// path from the machine that built viewr. It means nothing to the reader.
    fn sanitize_platform_detail(detail: &str) -> String {
        let sentence = detail
            .rsplit(": ")
            .find(|segment| {
                !segment.is_empty() && !segment.contains('/') && !segment.contains('\\')
            })
            .unwrap_or("the platform reported no reason");
        sentence.trim().chars().take(200).collect()
    }

    /// Launch-time message for an event loop that refused to start.
    pub(crate) fn event_loop_failure_message(detail: &str, compositor_unreachable: bool) -> String {
        let reason = sanitize_platform_detail(detail);
        if compositor_unreachable {
            return format!(
                "cannot open a window: WAYLAND_DISPLAY names a compositor socket that does not \
exist ({reason}).\nUnset WAYLAND_DISPLAY to use the X11 session named by DISPLAY, or start a \
desktop session.\n"
            );
        }
        format!(
            "cannot open a window: the desktop session refused to start an event loop \
({reason}).\nCheck that this session is a running desktop, then run viewr again.\n"
        )
    }

    /// What to do about a GPU surface that could not be created.
    ///
    /// Naming a package that is already installed wastes the reader's time, so
    /// an installed runtime moves the advice to the session itself.
    pub(crate) fn gpu_advice(egl: bool, vulkan: bool) -> String {
        if egl || vulkan {
            let present = match (egl, vulkan) {
                (true, true) => "EGL and Vulkan are",
                (true, false) => "EGL is",
                _ => "Vulkan is",
            };
            return format!(
                "{present} installed, so the graphics runtime is present and this display cannot \
present through it.\nA forwarded, nested, or virtual X session often cannot: try a local desktop \
session, or check that the display exposes EGL or a Vulkan driver.\n"
            );
        }
        let mut advice = String::from(
            "viewr renders through Vulkan or OpenGL, and neither runtime is installed here.\n\
Install one of them, then run viewr again:\n",
        );
        for library in GPU_RUNTIME_LIBRARIES {
            let _ = writeln!(advice, "  {}:", library.purpose);
            advice.push_str(&install_hint(library, "    "));
        }
        advice
    }

    /// Build the doctor window-presentation section for a Unix desktop.
    pub(crate) fn window_readiness_report(support: &WindowSupport) -> WindowReadiness {
        let mut lines = Vec::new();
        let mut critical_ok = true;
        match support.session {
            DisplaySession::None => {
                if support.compositor_unreachable {
                    lines.push(
                        "[WARN] display session: WAYLAND_DISPLAY names a compositor that is not running"
                            .to_owned(),
                    );
                } else {
                    lines.push(format!(
                        "[WARN] display session: {}",
                        support.session.summary()
                    ));
                }
                lines.push(
                    "       viewr cannot open a window here; CLI subcommands still work".to_owned(),
                );
            }
            DisplaySession::Wayland | DisplaySession::X11 => {
                lines.push(format!(
                    "[ok]   display session: {}",
                    support.session.summary()
                ));
                if support.compositor_unreachable {
                    lines.push(
                        "[note] WAYLAND_DISPLAY names a compositor that is not running; using X11"
                            .to_owned(),
                    );
                }
                if let Some(library) = support.missing_library {
                    critical_ok = false;
                    lines.push(format!(
                        "[FAIL] windowing library: {} is missing ({})",
                        library.sonames[0], library.purpose
                    ));
                    lines.extend(install_hint(library, "       ").lines().map(str::to_owned));
                } else {
                    let present: Vec<&str> = required_libraries(support.session)
                        .iter()
                        .map(|library| library.sonames[0])
                        .collect();
                    lines.push(format!(
                        "[ok]   windowing libraries: {} present",
                        present.join(", ")
                    ));
                }
                lines.extend(gpu_runtime_lines(
                    support.egl,
                    support.vulkan,
                    &mut critical_ok,
                ));
            }
        }
        lines.push(SURFACE_NOTE.to_owned());
        WindowReadiness { lines, critical_ok }
    }

    /// Report the graphics runtimes, failing when neither backend can exist.
    fn gpu_runtime_lines(egl: bool, vulkan: bool, critical_ok: &mut bool) -> Vec<String> {
        if !egl && !vulkan {
            *critical_ok = false;
            let mut lines = vec![
                "[FAIL] gpu runtime: no Vulkan or OpenGL runtime found, so no GPU surface can be created"
                    .to_owned(),
            ];
            for library in GPU_RUNTIME_LIBRARIES {
                lines.push(format!("       {}:", library.purpose));
                lines.extend(
                    install_hint(library, "         ")
                        .lines()
                        .map(str::to_owned),
                );
            }
            return lines;
        }
        let state = match (egl, vulkan) {
            (true, true) => "EGL and Vulkan present",
            (true, false) => "EGL present, Vulkan absent",
            _ => "Vulkan present, EGL absent",
        };
        vec![format!("[ok]   gpu runtime: {state}")]
    }

    #[cfg(test)]
    mod tests {
        use super::{
            DisplaySession, EGL, RequiredLibrary, VULKAN, WAYLAND_CLIENT, WindowSupport,
            XKBCOMMON_X11, compositor_missing_message, detect_session, event_loop_failure_message,
            fallback_session, gpu_advice, gpu_runtime_libraries, headless_message, install_hint,
            missing_library_message, required_libraries, sanitize_platform_detail,
            wayland_socket_path, window_readiness_report,
        };

        fn support(
            session: DisplaySession,
            missing_library: Option<&'static RequiredLibrary>,
            compositor_unreachable: bool,
            egl: bool,
            vulkan: bool,
        ) -> WindowSupport {
            WindowSupport {
                session,
                missing_library,
                compositor_unreachable,
                egl,
                vulkan,
            }
        }

        #[test]
        fn session_detection_prefers_a_reachable_compositor_and_honors_the_override() {
            assert_eq!(
                detect_session(None, Some("wayland-0"), Some(":0"), true),
                DisplaySession::Wayland
            );
            // A Wayland variable with no compositor behind it is not a session,
            // so an available X server runs instead of failing the launch.
            assert_eq!(
                detect_session(None, Some("wayland-0"), Some(":0"), false),
                DisplaySession::X11
            );
            assert_eq!(
                detect_session(None, Some("wayland-0"), None, false),
                DisplaySession::None
            );
            assert_eq!(
                detect_session(None, None, Some(":6"), false),
                DisplaySession::X11
            );
            assert_eq!(
                detect_session(None, Some(""), Some(""), false),
                DisplaySession::None
            );
            assert_eq!(
                detect_session(None, None, None, false),
                DisplaySession::None
            );
            // An explicit backend is respected even when it cannot work, so the
            // failure names the choice the user made.
            assert_eq!(
                detect_session(Some("wayland"), Some("wayland-0"), Some(":0"), false),
                DisplaySession::Wayland
            );
            assert_eq!(
                detect_session(Some("x11"), Some("wayland-0"), Some(":0"), true),
                DisplaySession::X11
            );
            assert_eq!(
                detect_session(Some("wayland"), None, Some(":0"), false),
                DisplaySession::None
            );
            assert_eq!(
                detect_session(Some("other"), None, Some(":0"), false),
                DisplaySession::X11
            );
        }

        #[test]
        fn the_wayland_socket_follows_the_protocol_rules() {
            assert_eq!(
                wayland_socket_path(Some("wayland-0"), Some("/run/user/1000")),
                Some("/run/user/1000/wayland-0".to_owned())
            );
            assert_eq!(
                wayland_socket_path(Some("wayland-0"), Some("/run/user/1000/")),
                Some("/run/user/1000/wayland-0".to_owned())
            );
            assert_eq!(
                wayland_socket_path(Some("/tmp/custom.sock"), None),
                Some("/tmp/custom.sock".to_owned())
            );
            assert_eq!(wayland_socket_path(Some("wayland-0"), None), None);
            assert_eq!(wayland_socket_path(Some(""), Some("/run/user/1000")), None);
            assert_eq!(wayland_socket_path(None, Some("/run/user/1000")), None);
        }

        #[test]
        fn an_incomplete_wayland_session_can_still_fall_back_to_x11() {
            assert_eq!(
                fallback_session(None, Some("wayland-0"), Some(":0")),
                DisplaySession::X11
            );
            assert_eq!(
                fallback_session(None, Some("wayland-0"), None),
                DisplaySession::None
            );
            assert_eq!(
                fallback_session(None, None, Some(":0")),
                DisplaySession::None
            );
            assert_eq!(fallback_session(None, None, None), DisplaySession::None);
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

            let runtimes: Vec<&str> = gpu_runtime_libraries()
                .iter()
                .map(|library| library.sonames[0])
                .collect();
            assert_eq!(runtimes, ["libEGL.so.1", "libvulkan.so.1"]);
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
        fn a_host_without_a_session_is_told_what_still_works() {
            for message in [headless_message(), compositor_missing_message()] {
                assert!(message.starts_with("cannot open a window: "));
                assert!(message.contains("viewr doctor"));
            }
            assert!(compositor_missing_message().contains("unset WAYLAND_DISPLAY"));
            assert!(headless_message().contains("no graphical session was found"));
        }

        #[test]
        fn platform_errors_keep_their_sentence_and_lose_the_build_machine_path() {
            let winit = "os error at /home/runner/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/winit-0.30.13/src/platform_impl/linux/wayland/event_loop/mod.rs:89: Could not find wayland compositor";
            assert_eq!(
                sanitize_platform_detail(winit),
                "Could not find wayland compositor"
            );
            assert_eq!(sanitize_platform_detail("plain failure"), "plain failure");
            assert_eq!(
                sanitize_platform_detail("/only/a/path"),
                "the platform reported no reason"
            );
            assert!(sanitize_platform_detail(&"x".repeat(500)).len() <= 200);

            let wayland = event_loop_failure_message(winit, true);
            assert!(!wayland.contains("/home/runner"));
            assert!(!wayland.contains(".cargo"));
            assert!(wayland.contains("Could not find wayland compositor"));
            assert!(wayland.contains("Unset WAYLAND_DISPLAY"));

            let other = event_loop_failure_message(winit, false);
            assert!(!other.contains("/home/runner"));
            assert!(other.contains("refused to start an event loop"));
        }

        #[test]
        fn gpu_advice_stops_recommending_a_runtime_that_is_already_installed() {
            let missing = gpu_advice(false, false);
            assert!(missing.contains("neither runtime is installed here"));
            assert!(missing.contains("sudo apt install libegl1 libegl-mesa0"));
            assert!(missing.contains("sudo apt install mesa-vulkan-drivers"));
            assert!(missing.contains("sudo dnf install mesa-libEGL"));
            assert!(missing.contains("sudo pacman -S vulkan-swrast"));

            for (egl, vulkan, expected) in [
                (true, true, "EGL and Vulkan are installed"),
                (true, false, "EGL is installed"),
                (false, true, "Vulkan is installed"),
            ] {
                let advice = gpu_advice(egl, vulkan);
                assert!(advice.contains(expected), "{egl} {vulkan}");
                assert!(advice.contains("forwarded, nested, or virtual X session"));
                assert!(!advice.contains("sudo apt install"));
            }
        }

        #[test]
        fn doctor_window_section_fails_when_this_host_cannot_present() {
            let ready =
                window_readiness_report(&support(DisplaySession::X11, None, false, true, true));
            assert!(ready.critical_ok);
            assert!(ready.lines[0].contains("display session: X11"));
            assert!(ready.lines[1].starts_with("[ok]   windowing libraries"));
            assert!(ready.lines[2] == "[ok]   gpu runtime: EGL and Vulkan present");

            let broken = window_readiness_report(&support(
                DisplaySession::X11,
                Some(&XKBCOMMON_X11),
                false,
                true,
                false,
            ));
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
            assert!(
                broken
                    .lines
                    .iter()
                    .any(|line| line == "[ok]   gpu runtime: EGL present, Vulkan absent")
            );

            // A session with no graphics runtime cannot present, and doctor
            // stops calling that a healthy install.
            let no_runtime =
                window_readiness_report(&support(DisplaySession::X11, None, false, false, false));
            assert!(!no_runtime.critical_ok);
            assert!(
                no_runtime
                    .lines
                    .iter()
                    .any(|line| line.starts_with("[FAIL] gpu runtime: no Vulkan or OpenGL runtime"))
            );
            assert!(
                no_runtime
                    .lines
                    .iter()
                    .any(|line| line.contains("sudo apt install libegl1 libegl-mesa0"))
            );

            // A stale Wayland variable explains itself instead of contradicting
            // the launch path.
            let stale =
                window_readiness_report(&support(DisplaySession::X11, None, true, true, false));
            assert!(stale.critical_ok);
            assert!(
                stale
                    .lines
                    .iter()
                    .any(|line| line.starts_with("[note] WAYLAND_DISPLAY names a compositor"))
            );

            let headless =
                window_readiness_report(&support(DisplaySession::None, None, false, false, false));
            assert!(headless.critical_ok);
            assert!(headless.lines[0].starts_with("[WARN] display session: none"));

            let stale_headless =
                window_readiness_report(&support(DisplaySession::None, None, true, true, true));
            assert!(stale_headless.critical_ok);
            assert!(stale_headless.lines[0].contains("compositor that is not running"));

            for report in [
                window_readiness_report(&support(DisplaySession::Wayland, None, false, true, true)),
                window_readiness_report(&support(DisplaySession::None, None, false, true, true)),
            ] {
                assert!(
                    report
                        .lines
                        .last()
                        .is_some_and(|line| line.contains("proven only when viewr opens a window"))
                );
            }

            assert_eq!(EGL.sonames[0], "libEGL.so.1");
            assert_eq!(VULKAN.sonames[0], "libvulkan.so.1");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NATIVE_GPU_ADVICE, gpu_failure_message, native_window_readiness};

    /// The loader probe and the launch decision, exercised on the real host.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_launch_check_asks_the_loader_and_agrees_with_its_own_report() {
        use super::{
            DisplaySession, first_missing_library, gpu_runtime_present, library_loads,
            preferred_backend, preflight, resolve_window_support,
        };

        assert!(library_loads("libc.so.6"));
        assert!(!library_loads("libviewr-does-not-exist.so.0"));
        assert!(!library_loads("interior\0nul.so"));

        let support = resolve_window_support();
        // A fallback is reported only when it resolves every library it needs,
        // so a reported gap always belongs to the reported backend.
        assert_eq!(
            support.missing_library.is_some(),
            first_missing_library(support.session).is_some()
        );
        assert_eq!(
            preflight().is_ok(),
            support.session != DisplaySession::None && support.missing_library.is_none()
        );
        assert_eq!(gpu_runtime_present(), (support.egl, support.vulkan));
        assert_eq!(
            preferred_backend().is_some(),
            support.session != DisplaySession::None
                && !std::env::var("WINIT_UNIX_BACKEND")
                    .is_ok_and(|value| matches!(value.trim(), "x11" | "wayland"))
        );
    }

    #[test]
    fn a_failed_surface_reports_the_reason_and_its_advice() {
        let message = gpu_failure_message("create_surface: no enabled backend", NATIVE_GPU_ADVICE);
        assert!(message.starts_with("cannot present images on this display: create_surface:"));
        assert!(message.contains("Update the graphics driver"));
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
