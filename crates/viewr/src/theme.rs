//! Appearance selection, color palettes, and the single persisted appearance
//! preference.
//!
//! System appearance still comes from winit. An explicit user choice is stored
//! as one validated word in the platform configuration directory. No image
//! path, window state, or activity history is written with it.

use std::fs;
use std::io::{self, Read, Write};
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
    /// Stable user-facing name for a resolved appearance.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Console => "Console",
        }
    }

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

/// Result of loading the optional appearance preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceLoad {
    /// A validated preference was restored.
    Loaded(Preference),
    /// No preference exists, which is the normal first-launch state.
    Missing,
    /// Unusable saved state was replaced in memory by the safe System default.
    Recovered(PreferenceRecovery),
}

impl PreferenceLoad {
    /// Preference to apply for this launch.
    #[must_use]
    pub const fn preference(self) -> Preference {
        match self {
            Self::Loaded(preference) => preference,
            Self::Missing | Self::Recovered(_) => Preference::System,
        }
    }

    /// Abnormal recovery category, if startup could not use saved state.
    #[must_use]
    pub const fn recovery(self) -> Option<PreferenceRecovery> {
        match self {
            Self::Recovered(recovery) => Some(recovery),
            Self::Loaded(_) | Self::Missing => None,
        }
    }
}

/// Path-private reason that saved appearance state could not be restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceRecovery {
    /// The bounded content was not a supported appearance word or valid UTF-8.
    Invalid,
    /// The saved value exceeded the fixed read contract.
    Oversized,
    /// The preference existed but could not be opened, inspected, or read.
    Unreadable,
    /// The platform did not provide a configuration directory.
    ConfigurationUnavailable,
}

impl PreferenceRecovery {
    /// Stable path-free label for explicitly enabled diagnostics.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Oversized => "oversized",
            Self::Unreadable => "unreadable",
            Self::ConfigurationUnavailable => "configuration-unavailable",
        }
    }

    /// Concise recovery status shown after the first window is ready.
    #[must_use]
    pub const fn notice(self) -> &'static str {
        match self {
            Self::Invalid | Self::Oversized | Self::Unreadable | Self::ConfigurationUnavailable => {
                "Could not restore saved appearance. Using System."
            }
        }
    }
}

/// Path-private phase in which an explicit appearance save failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceSaveError {
    /// The platform did not provide a configuration directory or usable path.
    ConfigurationUnavailable,
    /// The containing configuration directory could not be prepared.
    DirectoryUnavailable,
    /// A same-directory temporary file could not be created.
    TemporaryFileUnavailable,
    /// The complete preference word could not be written.
    WriteFailed,
    /// The completed temporary file could not be synchronized.
    SyncFailed,
    /// The synchronized temporary file could not atomically replace the preference.
    ReplaceFailed,
}

impl PreferenceSaveError {
    /// Stable path-free label for explicitly enabled diagnostics.
    #[must_use]
    pub const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::ConfigurationUnavailable => "configuration-unavailable",
            Self::DirectoryUnavailable => "directory-unavailable",
            Self::TemporaryFileUnavailable => "temporary-file-unavailable",
            Self::WriteFailed => "write-failed",
            Self::SyncFailed => "sync-failed",
            Self::ReplaceFailed => "replace-failed",
        }
    }
}

impl Preference {
    /// Every supported appearance choice, in menu order.
    pub const ALL: [Self; 4] = [Self::System, Self::Light, Self::Dark, Self::Console];

    /// Stable title shown before the outcome description.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Console => "Console",
        }
    }

    /// Concrete chooser copy for the visible effect of this preference.
    #[must_use]
    pub fn description(self, current_system_mode: Option<Mode>) -> String {
        match self {
            Self::System => match current_system_mode {
                Some(mode @ (Mode::Light | Mode::Dark)) => {
                    format!("Follows your operating system. Currently {}.", mode.name())
                }
                Some(Mode::Console) | None => {
                    "Follows your operating system's Light or Dark setting.".to_owned()
                }
            },
            Self::Light => {
                "Bright neutral chrome, light window frame, soft-white canvas.".to_owned()
            }
            Self::Dark => {
                "Low-glare charcoal chrome, dark window frame, deep-ink canvas.".to_owned()
            }
            Self::Console => {
                "Green-screen look, near-black canvas, phosphor-green chrome, monospaced type."
                    .to_owned()
            }
        }
    }

    /// Full semantic label used by assistive technology and native automation.
    #[must_use]
    pub fn accessible_label(self, current_system_mode: Option<Mode>) -> String {
        format!("{}: {}", self.name(), self.description(current_system_mode))
    }

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

/// Load the one-word appearance preference without rewriting unusable state.
///
/// Missing state is a quiet normal default. Unreadable, oversized, invalid, or
/// unavailable state is classified and safely resolves to [`Preference::System`].
#[must_use]
pub fn load_preference() -> PreferenceLoad {
    load_preference_from_path(preference_path().as_deref())
}

/// Persist one appearance word in the platform configuration directory.
///
/// # Errors
/// Returns a path-private phase category if no configuration directory is
/// available or the directory or file cannot be written, synced, or replaced.
pub fn save_preference(preference: Preference) -> Result<(), PreferenceSaveError> {
    let path = preference_path().ok_or(PreferenceSaveError::ConfigurationUnavailable)?;
    save_preference_to(&path, preference)
}

fn load_preference_from_path(path: Option<&Path>) -> PreferenceLoad {
    path.map_or(
        PreferenceLoad::Recovered(PreferenceRecovery::ConfigurationUnavailable),
        load_preference_from,
    )
}

fn load_preference_from(path: &Path) -> PreferenceLoad {
    let file = match crate::fs::open_file_no_atime(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return PreferenceLoad::Missing,
        Err(_) => return PreferenceLoad::Recovered(PreferenceRecovery::Unreadable),
    };
    let Ok(metadata) = file.metadata() else {
        return PreferenceLoad::Recovered(PreferenceRecovery::Unreadable);
    };
    if !metadata.is_file() {
        return PreferenceLoad::Recovered(PreferenceRecovery::Unreadable);
    }
    let length = metadata.len();
    if length > MAX_PREFERENCE_BYTES {
        return PreferenceLoad::Recovered(PreferenceRecovery::Oversized);
    }
    let mut bytes = Vec::with_capacity(length as usize);
    if file
        .take(MAX_PREFERENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return PreferenceLoad::Recovered(PreferenceRecovery::Unreadable);
    }
    if bytes.len() as u64 > MAX_PREFERENCE_BYTES {
        return PreferenceLoad::Recovered(PreferenceRecovery::Oversized);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return PreferenceLoad::Recovered(PreferenceRecovery::Invalid);
    };
    Preference::parse(value).map_or(
        PreferenceLoad::Recovered(PreferenceRecovery::Invalid),
        PreferenceLoad::Loaded,
    )
}

fn save_preference_to(path: &Path, preference: Preference) -> Result<(), PreferenceSaveError> {
    let parent = path
        .parent()
        .ok_or(PreferenceSaveError::ConfigurationUnavailable)?;
    fs::create_dir_all(parent).map_err(|_| PreferenceSaveError::DirectoryUnavailable)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| PreferenceSaveError::TemporaryFileUnavailable)?;
    temporary
        .write_all(format!("{}\n", preference.as_str()).as_bytes())
        .map_err(|_| PreferenceSaveError::WriteFailed)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| PreferenceSaveError::SyncFailed)?;
    temporary
        .persist(path)
        .map_err(|_| PreferenceSaveError::ReplaceFailed)?;
    Ok(())
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
        Mode, Preference, PreferenceLoad, PreferenceRecovery, PreferenceSaveError,
        chrome_palette_for, load_preference_from, load_preference_from_path, palette_for,
        save_preference_to,
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
    fn appearance_choices_describe_their_actual_outcomes() {
        assert_eq!(
            Preference::ALL,
            [
                Preference::System,
                Preference::Light,
                Preference::Dark,
                Preference::Console,
            ]
        );
        assert_eq!(
            Preference::System.accessible_label(Some(Mode::Light)),
            "System: Follows your operating system. Currently Light."
        );
        assert_eq!(
            Preference::System.accessible_label(Some(Mode::Dark)),
            "System: Follows your operating system. Currently Dark."
        );
        assert_eq!(
            Preference::Light.accessible_label(None),
            "Light: Bright neutral chrome, light window frame, soft-white canvas."
        );
        assert_eq!(
            Preference::Dark.accessible_label(None),
            "Dark: Low-glare charcoal chrome, dark window frame, deep-ink canvas."
        );
        assert_eq!(
            Preference::Console.accessible_label(None),
            "Console: Green-screen look, near-black canvas, phosphor-green chrome, monospaced type."
        );
        assert_eq!(
            Preference::System.accessible_label(None),
            "System: Follows your operating system's Light or Dark setting."
        );
        assert_eq!(
            Preference::System.accessible_label(Some(Mode::Console)),
            "System: Follows your operating system's Light or Dark setting."
        );
    }

    #[test]
    fn preference_round_trips_and_classifies_unusable_state() {
        let workspace = crate::ephemeral::TempWorkspace::new("appearance").unwrap();
        let path = workspace.path().join("nested").join("appearance");
        assert_eq!(load_preference_from(&path), PreferenceLoad::Missing);
        assert_eq!(PreferenceLoad::Missing.preference(), Preference::System);
        assert_eq!(PreferenceLoad::Missing.recovery(), None);
        assert_eq!(
            load_preference_from_path(None),
            PreferenceLoad::Recovered(PreferenceRecovery::ConfigurationUnavailable)
        );
        for preference in [
            Preference::System,
            Preference::Light,
            Preference::Dark,
            Preference::Console,
        ] {
            save_preference_to(&path, preference).unwrap();
            assert_eq!(
                load_preference_from(&path),
                PreferenceLoad::Loaded(preference)
            );
            let entries = std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(entries.len(), 1, "successful persistence left a temp file");
            assert_eq!(entries[0].path(), path);
        }
        std::fs::write(&path, "not-a-theme\n").unwrap();
        assert_eq!(
            load_preference_from(&path),
            PreferenceLoad::Recovered(PreferenceRecovery::Invalid)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not-a-theme\n");
        save_preference_to(&path, Preference::Console).unwrap();
        assert_eq!(
            load_preference_from(&path),
            PreferenceLoad::Loaded(Preference::Console)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "console\n");
        std::fs::write(&path, [0xFF, 0xFE]).unwrap();
        assert_eq!(
            load_preference_from(&path),
            PreferenceLoad::Recovered(PreferenceRecovery::Invalid)
        );
        std::fs::write(&path, "x".repeat(64)).unwrap();
        assert_eq!(
            load_preference_from(&path),
            PreferenceLoad::Recovered(PreferenceRecovery::Oversized)
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(
            load_preference_from(&path),
            PreferenceLoad::Recovered(PreferenceRecovery::Unreadable)
        );
    }

    #[test]
    fn recovery_copy_and_categories_are_fixed_and_path_private() {
        let recoveries = [
            (PreferenceRecovery::Invalid, "invalid"),
            (PreferenceRecovery::Oversized, "oversized"),
            (PreferenceRecovery::Unreadable, "unreadable"),
            (
                PreferenceRecovery::ConfigurationUnavailable,
                "configuration-unavailable",
            ),
        ];
        for (recovery, category) in recoveries {
            let load = PreferenceLoad::Recovered(recovery);
            assert_eq!(load.preference(), Preference::System);
            assert_eq!(load.recovery(), Some(recovery));
            assert_eq!(recovery.diagnostic_name(), category);
            assert_eq!(
                recovery.notice(),
                "Could not restore saved appearance. Using System."
            );
            assert!(
                !recovery
                    .diagnostic_name()
                    .chars()
                    .any(|character| matches!(character, '/' | '\\' | '\n' | '\r'))
            );
        }
    }

    #[test]
    fn save_failure_categories_are_fixed_and_path_private() {
        let failures = [
            (
                PreferenceSaveError::ConfigurationUnavailable,
                "configuration-unavailable",
            ),
            (
                PreferenceSaveError::DirectoryUnavailable,
                "directory-unavailable",
            ),
            (
                PreferenceSaveError::TemporaryFileUnavailable,
                "temporary-file-unavailable",
            ),
            (PreferenceSaveError::WriteFailed, "write-failed"),
            (PreferenceSaveError::SyncFailed, "sync-failed"),
            (PreferenceSaveError::ReplaceFailed, "replace-failed"),
        ];
        for (failure, category) in failures {
            assert_eq!(failure.diagnostic_name(), category);
            assert!(
                !failure
                    .diagnostic_name()
                    .chars()
                    .any(|character| matches!(character, '/' | '\\' | '\n' | '\r'))
            );
        }
    }
}
