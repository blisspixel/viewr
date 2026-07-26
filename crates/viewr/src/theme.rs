//! Appearance selection, color palettes, and the single persisted appearance
//! preference.
//!
//! System appearance still comes from winit. An explicit user choice is stored
//! as one validated word in the platform configuration directory. No image
//! path, window state, or activity history is written with it.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const MAX_PREFERENCE_BYTES: u64 = 32;

/// The resolved appearance presented by the renderer and application chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Bright neutral appearance.
    Light,
    /// Low-glare neutral appearance.
    Dark,
    /// Black and phosphor-green terminal-inspired appearance.
    Console,
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

    /// Choose a [`Mode`] from an optional OS theme, falling back to dark when
    /// the platform does not report one.
    #[must_use]
    pub fn from_winit_or_dark(theme: Option<winit::window::Theme>) -> Self {
        theme.map_or(Self::Dark, Self::from_winit)
    }
}

/// User-facing appearance preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    /// Follow operating-system light or dark appearance.
    #[default]
    System,
    /// Always use the light appearance.
    Light,
    /// Always use the dark appearance.
    Dark,
    /// Use the terminal-inspired console appearance.
    Console,
}

impl Preference {
    /// Native window-decoration override. System leaves the platform in
    /// control; Console uses dark decorations around its black chrome.
    #[must_use]
    pub const fn window_theme(self) -> Option<winit::window::Theme> {
        match self {
            Self::System => None,
            Self::Light => Some(winit::window::Theme::Light),
            Self::Dark | Self::Console => Some(winit::window::Theme::Dark),
        }
    }

    /// Resolve this preference against the currently reported OS theme.
    #[must_use]
    pub fn resolve(self, system_theme: Option<winit::window::Theme>) -> Mode {
        match self {
            Self::System => Mode::from_winit_or_dark(system_theme),
            Self::Light => Mode::Light,
            Self::Dark => Mode::Dark,
            Self::Console => Mode::Console,
        }
    }

    /// Stable lower-case value used by the local preference file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Console => "console",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "console" => Some(Self::Console),
            _ => None,
        }
    }
}

/// GPU background palette. Channels are sRGB values in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The window background shown behind and around images.
    pub background: [f64; 4],
}

/// UI colors used consistently by standard widgets and custom painting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChromePalette {
    /// Active and affirmative state color.
    pub accent: [u8; 4],
    /// Text color placed on an accent-filled control.
    pub accent_ink: [u8; 4],
    /// Primary text.
    pub text: [u8; 4],
    /// Secondary text that still meets the AA contrast floor.
    pub muted: [u8; 4],
    /// Primary panel surface.
    pub panel: [u8; 4],
    /// Raised and hovered surface.
    pub raised: [u8; 4],
    /// Pressed surface.
    pub active: [u8; 4],
    /// Panel and control outline.
    pub border: [u8; 4],
}

/// Return the image-background palette for a resolved mode.
#[must_use]
pub fn palette_for(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => Palette {
            background: [11.0 / 255.0, 14.0 / 255.0, 20.0 / 255.0, 1.0],
        },
        Mode::Light => Palette {
            background: [244.0 / 255.0, 245.0 / 255.0, 247.0 / 255.0, 1.0],
        },
        Mode::Console => Palette {
            background: [1.0 / 255.0, 5.0 / 255.0, 2.0 / 255.0, 1.0],
        },
    }
}

/// Return the complete chrome palette for a resolved mode.
#[must_use]
pub const fn chrome_palette_for(mode: Mode) -> ChromePalette {
    match mode {
        Mode::Dark => ChromePalette {
            accent: [0xF7, 0xA8, 0x45, 0xFF],
            accent_ink: [0x0B, 0x0E, 0x14, 0xFF],
            text: [0xE8, 0xED, 0xF3, 0xFF],
            muted: [0xB8, 0xC0, 0xCC, 0xFF],
            panel: [0x0F, 0x13, 0x1A, 0xFF],
            raised: [0x1A, 0x20, 0x2A, 0xFF],
            active: [0x25, 0x2D, 0x39, 0xFF],
            border: [0x2B, 0x33, 0x40, 0xFF],
        },
        Mode::Light => ChromePalette {
            accent: [0x98, 0x48, 0x00, 0xFF],
            accent_ink: [0xFF, 0xFF, 0xFF, 0xFF],
            text: [0x16, 0x1A, 0x20, 0xFF],
            muted: [0x4F, 0x59, 0x65, 0xFF],
            panel: [0xF7, 0xF8, 0xFA, 0xFF],
            raised: [0xE7, 0xEB, 0xF0, 0xFF],
            active: [0xD7, 0xDE, 0xE7, 0xFF],
            border: [0xB8, 0xC1, 0xCC, 0xFF],
        },
        Mode::Console => ChromePalette {
            accent: [0x45, 0xF5, 0x6A, 0xFF],
            accent_ink: [0x00, 0x15, 0x04, 0xFF],
            text: [0xB7, 0xF7, 0xBE, 0xFF],
            muted: [0x78, 0xB9, 0x82, 0xFF],
            panel: [0x03, 0x08, 0x04, 0xFF],
            raised: [0x0B, 0x1B, 0x0F, 0xFF],
            active: [0x12, 0x35, 0x1A, 0xFF],
            border: [0x23, 0x60, 0x2F, 0xFF],
        },
    }
}

/// Load the one-word appearance preference. Missing, unreadable, oversized, or
/// invalid files safely fall back to [`Preference::System`].
#[must_use]
pub fn load_preference() -> Preference {
    preference_path()
        .and_then(|path| load_preference_from(&path).ok())
        .flatten()
        .unwrap_or_default()
}

/// Persist one appearance word in the platform configuration directory.
///
/// # Errors
/// Returns an I/O error if no configuration directory is available or the
/// directory or file cannot be written.
pub fn save_preference(preference: Preference) -> io::Result<()> {
    let path = preference_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "platform configuration directory is unavailable",
        )
    })?;
    save_preference_to(&path, preference)
}

fn load_preference_from(path: &Path) -> io::Result<Option<Preference>> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > MAX_PREFERENCE_BYTES {
        return Ok(None);
    }
    let mut value = String::new();
    file.take(MAX_PREFERENCE_BYTES + 1)
        .read_to_string(&mut value)?;
    Ok(Preference::parse(&value))
}

fn save_preference_to(path: &Path, preference: Preference) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "appearance preference path has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    fs::write(path, format!("{}\n", preference.as_str()))
}

fn preference_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("viewr").join("appearance"));
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("viewr")
                    .join("appearance")
            });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Some(path.join("viewr").join("appearance"));
        }
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("viewr").join("appearance"));
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::{
        Mode, Preference, chrome_palette_for, load_preference_from, palette_for, save_preference_to,
    };

    #[test]
    fn dark_and_console_are_darker_than_light() {
        let sum = |color: [f64; 4]| color[0] + color[1] + color[2];
        assert!(sum(palette_for(Mode::Dark).background) < sum(palette_for(Mode::Light).background));
        assert!(
            sum(palette_for(Mode::Console).background) < sum(palette_for(Mode::Light).background)
        );
    }

    #[test]
    fn palettes_are_opaque() {
        for mode in [Mode::Light, Mode::Dark, Mode::Console] {
            assert!((palette_for(mode).background[3] - 1.0).abs() < f64::EPSILON);
            let chrome = chrome_palette_for(mode);
            for color in [
                chrome.accent,
                chrome.accent_ink,
                chrome.text,
                chrome.muted,
                chrome.panel,
                chrome.raised,
                chrome.active,
                chrome.border,
            ] {
                assert_eq!(color[3], 255);
            }
        }
    }

    #[test]
    fn maps_and_resolves_winit_theme() {
        use winit::window::Theme;
        assert_eq!(Mode::from_winit(Theme::Light), Mode::Light);
        assert_eq!(Mode::from_winit(Theme::Dark), Mode::Dark);
        assert_eq!(Preference::System.resolve(Some(Theme::Light)), Mode::Light);
        assert_eq!(Preference::Dark.resolve(Some(Theme::Light)), Mode::Dark);
        assert_eq!(Preference::Console.resolve(None), Mode::Console);
        assert_eq!(Preference::System.window_theme(), None);
        assert_eq!(Preference::Light.window_theme(), Some(Theme::Light));
        assert_eq!(Preference::Console.window_theme(), Some(Theme::Dark));
    }

    #[test]
    fn system_falls_back_to_dark_when_unknown() {
        assert_eq!(Mode::from_winit_or_dark(None), Mode::Dark);
        assert_eq!(Preference::System.resolve(None), Mode::Dark);
    }

    #[test]
    fn preference_round_trips_and_rejects_invalid_or_oversized_values() {
        let workspace = crate::ephemeral::TempWorkspace::new("appearance").unwrap();
        let path = workspace.path().join("nested").join("appearance");
        for preference in [
            Preference::System,
            Preference::Light,
            Preference::Dark,
            Preference::Console,
        ] {
            save_preference_to(&path, preference).unwrap();
            assert_eq!(load_preference_from(&path).unwrap(), Some(preference));
        }
        std::fs::write(&path, "not-a-theme\n").unwrap();
        assert_eq!(load_preference_from(&path).unwrap(), None);
        std::fs::write(&path, "x".repeat(64)).unwrap();
        assert_eq!(load_preference_from(&path).unwrap(), None);
    }
}
