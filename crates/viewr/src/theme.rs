//! Appearance handling: map the operating system light/dark setting to viewr's
//! color palette. The OS setting is read through winit (`Window::theme`), so we
//! need no extra dependency for it. The mapping here is pure and testable.

/// The light or dark appearance viewr should present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Light appearance.
    Light,
    /// Dark appearance.
    Dark,
}

impl Mode {
    /// Convert winit's reported OS theme into a viewr [`Mode`].
    #[must_use]
    pub fn from_winit(theme: winit::window::Theme) -> Self {
        match theme {
            winit::window::Theme::Light => Self::Light,
            winit::window::Theme::Dark => Self::Dark,
        }
    }

    /// Choose a [`Mode`] from an optional OS theme, falling back to
    /// [`Mode::Dark`] when the platform does not report one. A dark default
    /// keeps the viewer calm and glare-free.
    #[must_use]
    pub fn from_winit_or_dark(theme: Option<winit::window::Theme>) -> Self {
        theme.map_or(Self::Dark, Self::from_winit)
    }
}

/// A viewr color palette. Channels are sRGB values in `0.0..=1.0`, matching what
/// the GPU surface expects for its clear color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The window background, shown behind and around images.
    pub background: [f64; 4],
}

/// Return the palette for the given [`Mode`].
#[must_use]
pub fn palette_for(mode: Mode) -> Palette {
    match mode {
        // Deep ink, matching the app icon tile (#0B0E14).
        Mode::Dark => Palette {
            background: [0.043, 0.055, 0.078, 1.0],
        },
        // Soft off-white (#EEF1F5).
        Mode::Light => Palette {
            background: [0.933, 0.945, 0.960, 1.0],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, palette_for};

    #[test]
    fn dark_is_darker_than_light() {
        let sum = |c: [f64; 4]| c[0] + c[1] + c[2];
        assert!(sum(palette_for(Mode::Dark).background) < sum(palette_for(Mode::Light).background));
    }

    #[test]
    fn palettes_are_opaque() {
        assert!((palette_for(Mode::Dark).background[3] - 1.0).abs() < f64::EPSILON);
        assert!((palette_for(Mode::Light).background[3] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn maps_winit_theme() {
        use winit::window::Theme;
        assert_eq!(Mode::from_winit(Theme::Light), Mode::Light);
        assert_eq!(Mode::from_winit(Theme::Dark), Mode::Dark);
    }

    #[test]
    fn falls_back_to_dark_when_unknown() {
        assert_eq!(Mode::from_winit_or_dark(None), Mode::Dark);
        assert_eq!(
            Mode::from_winit_or_dark(Some(winit::window::Theme::Light)),
            Mode::Light
        );
    }
}
