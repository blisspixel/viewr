//! Pure monitor-identity comparison and display-color policy.
//!
//! The event loop owns the window, winit geometry, and redraw. This module
//! decides whether two observations name the same display, whether a display
//! ICC may ever be applied, and how the tagged-sRGB swapchain should be
//! described. It never fetches platform profiles or writes display-referred
//! pixels.

/// Physical geometry winit reports for one monitor.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MonitorExtent {
    pub origin: (i32, i32),
    pub physical_size: (u32, u32),
    pub scale_factor: f64,
}

/// Stable identity for the display that currently contains the window.
///
/// Name is optional and not unique. Equality is name + extent so a same-named
/// replacement at a new origin is a different display.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MonitorIdentity {
    name: Option<Box<str>>,
    extent: MonitorExtent,
}

/// What changed since the last observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayObservation {
    /// Same monitor identity as last time.
    Unchanged,
    /// First observation, or a different monitor.
    IdentityChanged,
    /// A previously known monitor disappeared.
    Unknown,
}

/// How the current swapchain should be described.
///
/// This increment never applies a display transform. Every variant presents
/// tagged sRGB. The distinction is why. The `Srgb` prefix is the contract, not
/// a repeated type name.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayOutputStatus {
    /// sRGB surface; the operating system maps to the display when it can.
    SrgbOperatingSystem,
    /// A display ICC was admitted but is not applied to pixels.
    SrgbDisplayProfileRecorded,
    /// No monitor identity or no usable profile; deterministic sRGB with no display claim.
    SrgbFallback,
}

impl DisplayOutputStatus {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::SrgbOperatingSystem => "sRGB, operating-system managed",
            Self::SrgbDisplayProfileRecorded => "sRGB, display profile recorded",
            Self::SrgbFallback => "sRGB fallback",
        }
    }
}

/// Host operating system used only to choose a color-management policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayOs {
    Windows,
    Macos,
    Linux,
    Other,
}

/// Window-system class that decides whether the compositor already converts sRGB.
///
/// Linux constructs the compositor classes at runtime. Windows and macOS store
/// `Native` and still match the others in the shared policy table.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplaySession {
    /// Not yet classified; treated as compositor-managed so we do not apply ICC.
    Unknown,
    Wayland,
    Xwayland,
    X11,
    Native,
}

/// The windowing backend the process actually presents through on Linux.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxWindowBackend {
    Wayland,
    X11,
    None,
}

/// Facts the event loop can supply without parsing an ICC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DisplayHints {
    pub os: DisplayOs,
    /// Windows Advanced Color / auto color management, when known.
    pub advanced_color: Option<bool>,
    pub session: DisplaySession,
}

/// Whether this process should ever apply a display ICC to pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayColorPolicy {
    /// Present tagged sRGB and let the operating system or compositor convert.
    OsManagedSrgb,
    /// The platform does not convert sRGB; a later slice may apply the ICC.
    LegacyDisplayIcc,
    /// No trustworthy display; keep the deterministic sRGB fallback.
    UnavailableSrgbFallback,
}

/// Map the windowing backend launch already resolved onto the color-session class.
///
/// A live Wayland compositor with an X11 window is Xwayland, which still color-
/// manages. A dead `WAYLAND_DISPLAY` that fell back to X11 is real X11.
#[cfg(any(target_os = "linux", test))]
#[must_use]
pub(crate) const fn linux_session(
    backend: LinuxWindowBackend,
    wayland_compositor_reachable: bool,
) -> DisplaySession {
    match backend {
        LinuxWindowBackend::Wayland => DisplaySession::Wayland,
        LinuxWindowBackend::X11 if wayland_compositor_reachable => DisplaySession::Xwayland,
        LinuxWindowBackend::X11 => DisplaySession::X11,
        LinuxWindowBackend::None => DisplaySession::Unknown,
    }
}

/// Compile-time host, used when the event loop has no richer probe yet.
#[must_use]
pub(crate) const fn current_os() -> DisplayOs {
    if cfg!(target_os = "windows") {
        DisplayOs::Windows
    } else if cfg!(target_os = "macos") {
        DisplayOs::Macos
    } else if cfg!(target_os = "linux") {
        DisplayOs::Linux
    } else {
        DisplayOs::Other
    }
}

/// Choose a presentation policy. Unknown facts fail toward "do not apply ICC".
#[must_use]
pub(crate) fn display_color_policy(
    hints: DisplayHints,
    monitor: Option<&MonitorIdentity>,
    profile_usable: bool,
) -> DisplayColorPolicy {
    if monitor.is_none() {
        return DisplayColorPolicy::UnavailableSrgbFallback;
    }
    match hints.os {
        DisplayOs::Windows => match hints.advanced_color {
            Some(false) if profile_usable => DisplayColorPolicy::LegacyDisplayIcc,
            Some(false) => DisplayColorPolicy::UnavailableSrgbFallback,
            Some(true) | None => DisplayColorPolicy::OsManagedSrgb,
        },
        DisplayOs::Linux => match hints.session {
            DisplaySession::X11 if profile_usable => DisplayColorPolicy::LegacyDisplayIcc,
            DisplaySession::X11 => DisplayColorPolicy::UnavailableSrgbFallback,
            DisplaySession::Wayland
            | DisplaySession::Xwayland
            | DisplaySession::Unknown
            | DisplaySession::Native => DisplayColorPolicy::OsManagedSrgb,
        },
        DisplayOs::Macos | DisplayOs::Other => DisplayColorPolicy::OsManagedSrgb,
    }
}

/// Admit display ICC bytes without building a pixel transform.
///
/// The event loop does not fetch platform profile bytes yet. Tests already
/// prove the bound, RGB, and rejection cases so the next slice cannot relax
/// them.
#[allow(dead_code)]
#[must_use]
pub(crate) fn admit_display_profile(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.len() > viewr_protocol::MAX_COLOR_PROFILE_BYTES {
        return false;
    }
    let Ok(profile) = moxcms::ColorProfile::new_from_slice(bytes) else {
        return false;
    };
    profile.color_space == moxcms::DataColorSpace::Rgb
}

/// Map a policy onto the user-visible swapchain description.
#[must_use]
pub(crate) const fn status_for_policy(policy: DisplayColorPolicy) -> DisplayOutputStatus {
    match policy {
        DisplayColorPolicy::OsManagedSrgb => DisplayOutputStatus::SrgbOperatingSystem,
        DisplayColorPolicy::LegacyDisplayIcc => DisplayOutputStatus::SrgbDisplayProfileRecorded,
        DisplayColorPolicy::UnavailableSrgbFallback => DisplayOutputStatus::SrgbFallback,
    }
}

/// Build an identity from winit geometry, or reject a report that cannot name a display.
///
/// Empty and whitespace-only names are stored as `None` so two unnamed reports
/// with the same extent compare equal. A non-empty name is trimmed so surrounding
/// space does not invent a new display.
#[must_use]
pub(crate) fn monitor_identity(
    name: Option<&str>,
    origin: (i32, i32),
    physical_size: (u32, u32),
    scale_factor: f64,
) -> Option<MonitorIdentity> {
    if !scale_factor.is_finite()
        || scale_factor <= 0.0
        || physical_size.0 == 0
        || physical_size.1 == 0
    {
        return None;
    }
    let name = name.and_then(|name| {
        let trimmed = name.trim();
        (!trimmed.is_empty()).then(|| Box::<str>::from(trimmed))
    });
    Some(MonitorIdentity {
        name,
        extent: MonitorExtent {
            origin,
            physical_size,
            scale_factor,
        },
    })
}

/// Compare the last stored identity with the current observation.
///
/// Two missing monitors stay `Unchanged` so a compositor that never names a
/// display cannot turn every move into a redraw. Losing a previously known
/// monitor is `Unknown`.
#[must_use]
pub(crate) fn observe_display(
    previous: Option<&MonitorIdentity>,
    current: Option<&MonitorIdentity>,
) -> DisplayObservation {
    match (previous, current) {
        (None, None) => DisplayObservation::Unchanged,
        (Some(_), None) => DisplayObservation::Unknown,
        (Some(previous), Some(current)) if previous == current => DisplayObservation::Unchanged,
        (None | Some(_), Some(_)) => DisplayObservation::IdentityChanged,
    }
}

/// Describe the tagged-sRGB swapchain from monitor identity and host facts.
///
/// Unknown ACM or session facts still fail toward compositor-managed sRGB so
/// this path cannot apply a display ICC. A known unmanaged X11 session without
/// an admitted profile reports the deterministic fallback instead.
#[must_use]
pub(crate) fn output_status(
    current: Option<&MonitorIdentity>,
    hints: DisplayHints,
    profile_usable: bool,
) -> DisplayOutputStatus {
    status_for_policy(display_color_policy(hints, current, profile_usable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(
        name: Option<&str>,
        origin: (i32, i32),
        physical_size: (u32, u32),
        scale_factor: f64,
    ) -> MonitorIdentity {
        monitor_identity(name, origin, physical_size, scale_factor).expect("valid test monitor")
    }

    #[test]
    fn invalid_scale_or_zero_extent_rejects_identity() {
        let origin = (0, 0);
        let size = (1920, 1080);
        for scale in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0, -0.0] {
            assert!(
                monitor_identity(Some("Desk"), origin, size, scale).is_none(),
                "accepted invalid scale {scale:?}"
            );
        }
        assert!(monitor_identity(Some("Desk"), origin, (0, 1080), 1.0).is_none());
        assert!(monitor_identity(Some("Desk"), origin, (1920, 0), 1.0).is_none());
        assert!(monitor_identity(Some("Desk"), origin, (0, 0), 1.0).is_none());
    }

    #[test]
    fn empty_or_whitespace_name_is_stored_as_none() {
        let named = identity(Some("  Desk  "), (0, 0), (1920, 1080), 1.0);
        assert_eq!(named.name.as_deref(), Some("Desk"));

        for name in [None, Some(""), Some("   "), Some("\t\n")] {
            let unnamed = identity(name, (0, 0), (1920, 1080), 1.0);
            assert_eq!(unnamed.name, None);
            assert_eq!(
                observe_display(
                    Some(&identity(None, (0, 0), (1920, 1080), 1.0)),
                    Some(&unnamed)
                ),
                DisplayObservation::Unchanged
            );
        }
    }

    #[test]
    fn same_name_and_extent_is_unchanged() {
        let previous = identity(Some("Desk"), (0, 0), (1920, 1080), 1.25);
        let current = identity(Some("Desk"), (0, 0), (1920, 1080), 1.25);
        assert_eq!(
            observe_display(Some(&previous), Some(&current)),
            DisplayObservation::Unchanged
        );
        assert_eq!(previous.extent, current.extent);
    }

    #[test]
    fn different_origin_size_name_or_scale_is_identity_changed() {
        let previous = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        let cases = [
            identity(Some("Desk"), (1920, 0), (1920, 1080), 1.0),
            identity(Some("Desk"), (0, 0), (2560, 1440), 1.0),
            identity(Some("Laptop"), (0, 0), (1920, 1080), 1.0),
            identity(Some("Desk"), (0, 0), (1920, 1080), 2.0),
        ];
        for current in cases {
            assert_eq!(
                observe_display(Some(&previous), Some(&current)),
                DisplayObservation::IdentityChanged,
                "treated distinct display as unchanged: {current:?}"
            );
        }
    }

    #[test]
    fn missing_current_monitor_is_unknown_only_after_a_known_display() {
        let previous = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        assert_eq!(observe_display(None, None), DisplayObservation::Unchanged);
        assert_eq!(
            observe_display(Some(&previous), None),
            DisplayObservation::Unknown
        );
    }

    #[test]
    fn first_known_monitor_is_identity_changed() {
        let current = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        assert_eq!(
            observe_display(None, Some(&current)),
            DisplayObservation::IdentityChanged
        );
    }

    #[test]
    fn output_status_follows_whether_a_monitor_is_identified() {
        let current = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        let managed = hints(DisplayOs::Windows, None, DisplaySession::Native);
        assert_eq!(
            output_status(Some(&current), managed, false),
            DisplayOutputStatus::SrgbOperatingSystem
        );
        assert_eq!(
            output_status(None, managed, false),
            DisplayOutputStatus::SrgbFallback
        );
    }

    #[test]
    fn unmanaged_x11_without_a_profile_reports_srgb_fallback() {
        let current = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        assert_eq!(
            output_status(
                Some(&current),
                hints(DisplayOs::Linux, None, DisplaySession::X11),
                false,
            ),
            DisplayOutputStatus::SrgbFallback
        );
        assert_eq!(
            output_status(
                Some(&current),
                hints(DisplayOs::Linux, None, DisplaySession::X11),
                true,
            ),
            DisplayOutputStatus::SrgbDisplayProfileRecorded
        );
        assert_eq!(
            output_status(
                Some(&current),
                hints(DisplayOs::Linux, None, DisplaySession::Wayland),
                false,
            ),
            DisplayOutputStatus::SrgbOperatingSystem
        );
    }

    #[test]
    fn linux_session_maps_the_resolved_window_backend() {
        assert_eq!(
            linux_session(LinuxWindowBackend::Wayland, true),
            DisplaySession::Wayland
        );
        assert_eq!(
            linux_session(LinuxWindowBackend::X11, true),
            DisplaySession::Xwayland
        );
        assert_eq!(
            linux_session(LinuxWindowBackend::X11, false),
            DisplaySession::X11
        );
        assert_eq!(
            linux_session(LinuxWindowBackend::None, false),
            DisplaySession::Unknown
        );
        assert_eq!(
            linux_session(LinuxWindowBackend::None, true),
            DisplaySession::Unknown
        );
    }

    #[test]
    fn output_status_labels_are_honest_srgb_copy() {
        assert_eq!(
            DisplayOutputStatus::SrgbOperatingSystem.label(),
            "sRGB, operating-system managed"
        );
        assert_eq!(
            DisplayOutputStatus::SrgbDisplayProfileRecorded.label(),
            "sRGB, display profile recorded"
        );
        assert_eq!(DisplayOutputStatus::SrgbFallback.label(), "sRGB fallback");
    }

    fn hints(os: DisplayOs, advanced_color: Option<bool>, session: DisplaySession) -> DisplayHints {
        DisplayHints {
            os,
            advanced_color,
            session,
        }
    }

    #[test]
    fn display_policy_fails_closed_without_a_monitor() {
        let desk = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        for os in [
            DisplayOs::Windows,
            DisplayOs::Macos,
            DisplayOs::Linux,
            DisplayOs::Other,
        ] {
            assert_eq!(
                display_color_policy(hints(os, Some(false), DisplaySession::X11), None, true),
                DisplayColorPolicy::UnavailableSrgbFallback,
                "{os:?} applied a display policy without a monitor"
            );
        }
        assert_eq!(
            display_color_policy(
                hints(DisplayOs::Macos, None, DisplaySession::Native),
                Some(&desk),
                false,
            ),
            DisplayColorPolicy::OsManagedSrgb
        );
    }

    #[test]
    fn display_policy_never_applies_icc_on_managed_compositors() {
        let desk = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        let managed = [
            hints(DisplayOs::Macos, None, DisplaySession::Native),
            hints(DisplayOs::Windows, Some(true), DisplaySession::Native),
            hints(DisplayOs::Windows, None, DisplaySession::Native),
            hints(DisplayOs::Linux, None, DisplaySession::Wayland),
            hints(DisplayOs::Linux, None, DisplaySession::Xwayland),
            hints(DisplayOs::Linux, None, DisplaySession::Unknown),
        ];
        for hint in managed {
            assert_eq!(
                display_color_policy(hint, Some(&desk), true),
                DisplayColorPolicy::OsManagedSrgb,
                "{hint:?} would apply a display ICC on a managed compositor"
            );
        }
    }

    #[test]
    fn display_policy_records_legacy_icc_only_when_unmanaged_and_usable() {
        let desk = identity(Some("Desk"), (0, 0), (1920, 1080), 1.0);
        assert_eq!(
            display_color_policy(
                hints(DisplayOs::Windows, Some(false), DisplaySession::Native),
                Some(&desk),
                true,
            ),
            DisplayColorPolicy::LegacyDisplayIcc
        );
        assert_eq!(
            display_color_policy(
                hints(DisplayOs::Linux, None, DisplaySession::X11),
                Some(&desk),
                true,
            ),
            DisplayColorPolicy::LegacyDisplayIcc
        );
        assert_eq!(
            display_color_policy(
                hints(DisplayOs::Windows, Some(false), DisplaySession::Native),
                Some(&desk),
                false,
            ),
            DisplayColorPolicy::UnavailableSrgbFallback
        );
        assert_eq!(
            status_for_policy(DisplayColorPolicy::LegacyDisplayIcc),
            DisplayOutputStatus::SrgbDisplayProfileRecorded
        );
    }

    #[test]
    fn display_profile_bytes_are_admitted_only_when_bounded_rgb() {
        let srgb = moxcms::ColorProfile::new_srgb()
            .encode()
            .expect("encode sRGB display fixture");
        assert!(admit_display_profile(&srgb));
        assert!(!admit_display_profile(&[]));
        assert!(!admit_display_profile(b"not an ICC profile"));
        assert!(!admit_display_profile(&vec![
            0;
            viewr_protocol::MAX_COLOR_PROFILE_BYTES
                + 1
        ]));

        let mut cmyk = moxcms::ColorProfile::new_srgb();
        cmyk.color_space = moxcms::DataColorSpace::Cmyk;
        let encoded = cmyk.encode().expect("encode CMYK display fixture");
        assert!(!admit_display_profile(&encoded));

        let gray = moxcms::ColorProfile::new_gray_with_gamma(2.2)
            .encode()
            .expect("encode gray display fixture");
        assert!(!admit_display_profile(&gray));
    }

    #[test]
    fn returning_to_a_prior_monitor_is_a_new_identity() {
        let monitor_a = identity(Some("A"), (0, 0), (1920, 1080), 1.0);
        let monitor_b = identity(Some("B"), (1920, 0), (2560, 1440), 1.25);
        assert_eq!(
            observe_display(Some(&monitor_a), Some(&monitor_b)),
            DisplayObservation::IdentityChanged
        );
        assert_eq!(
            observe_display(Some(&monitor_b), Some(&monitor_a)),
            DisplayObservation::IdentityChanged
        );
        assert_eq!(
            observe_display(Some(&monitor_a), Some(&monitor_a)),
            DisplayObservation::Unchanged
        );
    }
}
