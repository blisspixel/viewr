//! Operating-system display accessibility settings: high contrast and text size.
//!
//! The platform reads live in `display_probe` and return raw, untrusted values.
//! This module owns what those values mean: validation, clamping, and the safe
//! fallback of normal contrast at 100 percent text when a setting is missing,
//! malformed, or unreadable. Nothing here is persisted.

/// Smallest interface text scale viewr applies. Settings that ask for smaller
/// text keep the display-scale default rather than shrinking the interface.
pub(crate) const MIN_TEXT_SCALE: f32 = 1.0;
/// Largest interface text scale viewr applies, matching the Windows maximum.
pub(crate) const MAX_TEXT_SCALE: f32 = 2.25;

/// Raw readings from the platform. `None` means the platform has no such
/// setting or the read failed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SystemAccessibilityReading {
    /// Whether a high-contrast or increased-contrast mode is on.
    pub high_contrast: Option<bool>,
    /// Requested text scale as a plain factor, 1.0 for 100 percent.
    pub text_scale: Option<f64>,
}

/// Settings viewr applies to its interface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SystemAccessibility {
    /// Use a high-contrast chrome palette while Appearance follows the system.
    pub high_contrast: bool,
    /// Interface zoom on top of the display scale, within the supported range.
    pub text_scale: f32,
}

impl Default for SystemAccessibility {
    fn default() -> Self {
        Self {
            high_contrast: false,
            text_scale: MIN_TEXT_SCALE,
        }
    }
}

impl SystemAccessibility {
    /// Apply validation and fallback to one platform reading.
    #[must_use]
    pub(crate) fn from_reading(reading: SystemAccessibilityReading) -> Self {
        Self {
            high_contrast: reading.high_contrast.unwrap_or(false),
            text_scale: reading.text_scale.map_or(MIN_TEXT_SCALE, clamp_text_scale),
        }
    }
}

/// The interface zoom to apply in a window of `window` logical points.
///
/// The chrome layout is verified at the `minimum` window size. Enlarged text
/// may use only as much zoom as keeps at least that many points available in
/// both directions, so a larger text size never pushes menus or controls out
/// of reach; it grows to the requested size as the window does.
#[must_use]
pub(crate) fn effective_interface_scale(
    requested: f32,
    window: (f64, f64),
    minimum: (f64, f64),
) -> f32 {
    let room = (window.0 / minimum.0).min(window.1 / minimum.1);
    if !room.is_finite() || room <= f64::from(MIN_TEXT_SCALE) {
        return MIN_TEXT_SCALE;
    }
    #[allow(clippy::cast_possible_truncation)]
    let room = room.min(f64::from(MAX_TEXT_SCALE)) as f32;
    requested.clamp(MIN_TEXT_SCALE, room)
}

/// Clamp a requested text scale into the supported range. A non-finite or
/// non-positive request is treated as unreadable and keeps 100 percent.
#[must_use]
pub(crate) fn clamp_text_scale(requested: f64) -> f32 {
    if !requested.is_finite() || requested <= 0.0 {
        return MIN_TEXT_SCALE;
    }
    #[allow(clippy::cast_possible_truncation)]
    let requested = requested.clamp(f64::from(MIN_TEXT_SCALE), f64::from(MAX_TEXT_SCALE)) as f32;
    requested
}

/// Windows stores text size as a whole percentage from 100 to 225.
#[cfg(any(windows, test))]
#[must_use]
pub(crate) fn text_scale_from_percent(percent: u32) -> f64 {
    f64::from(percent) / 100.0
}

/// The XDG desktop portal reports `org.freedesktop.appearance` `contrast` as
/// 0 for no preference and 1 for high contrast. Any other value is unknown and
/// is not treated as a request.
#[cfg(any(target_os = "linux", test))]
#[must_use]
pub(crate) const fn portal_contrast_is_high(contrast: u32) -> bool {
    contrast == 1
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_TEXT_SCALE, MIN_TEXT_SCALE, SystemAccessibility, SystemAccessibilityReading,
        clamp_text_scale, effective_interface_scale, portal_contrast_is_high,
        text_scale_from_percent,
    };

    fn assert_scale(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < f32::EPSILON,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn missing_or_unreadable_settings_keep_normal_contrast_and_text() {
        assert_eq!(
            SystemAccessibility::from_reading(SystemAccessibilityReading::default()),
            SystemAccessibility::default()
        );
        for unreadable in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.5] {
            assert_scale(clamp_text_scale(unreadable), MIN_TEXT_SCALE);
        }
    }

    #[test]
    fn text_scale_is_clamped_to_the_supported_range() {
        assert_scale(clamp_text_scale(0.8), MIN_TEXT_SCALE);
        assert_scale(clamp_text_scale(1.5), 1.5);
        assert_scale(clamp_text_scale(3.0), MAX_TEXT_SCALE);
        assert_scale(clamp_text_scale(text_scale_from_percent(125)), 1.25);
        assert_scale(
            clamp_text_scale(text_scale_from_percent(225)),
            MAX_TEXT_SCALE,
        );
        assert_scale(clamp_text_scale(text_scale_from_percent(0)), MIN_TEXT_SCALE);
    }

    #[test]
    fn a_reading_maps_to_the_applied_settings() {
        let applied = SystemAccessibility::from_reading(SystemAccessibilityReading {
            high_contrast: Some(true),
            text_scale: Some(1.75),
        });
        assert!(applied.high_contrast);
        assert_scale(applied.text_scale, 1.75);
    }

    #[test]
    fn enlarged_text_keeps_the_verified_minimum_layout_in_reach() {
        let minimum = (640.0, 480.0);
        // A minimum-size window has no room to enlarge.
        assert_scale(
            effective_interface_scale(2.25, (640.0, 480.0), minimum),
            MIN_TEXT_SCALE,
        );
        // The narrower direction limits the zoom.
        assert_scale(
            effective_interface_scale(2.25, (1280.0, 720.0), minimum),
            1.5,
        );
        // With room, the full request applies, and never beyond it.
        assert_scale(
            effective_interface_scale(1.25, (1920.0, 1080.0), minimum),
            1.25,
        );
        assert_scale(
            effective_interface_scale(2.25, (3840.0, 2160.0), minimum),
            MAX_TEXT_SCALE,
        );
        // Degenerate window sizes keep 100 percent.
        for window in [(0.0, 0.0), (f64::NAN, 480.0), (100.0, 100.0)] {
            assert_scale(
                effective_interface_scale(2.0, window, minimum),
                MIN_TEXT_SCALE,
            );
        }
    }

    #[test]
    fn only_the_documented_portal_value_requests_high_contrast() {
        assert!(!portal_contrast_is_high(0));
        assert!(portal_contrast_is_high(1));
        assert!(!portal_contrast_is_high(2));
        assert!(!portal_contrast_is_high(u32::MAX));
    }
}
