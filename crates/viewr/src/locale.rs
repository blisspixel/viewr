//! Offline user-interface language selection and bounded persistence.
//!
//! System locale discovery reads only operating-system process state. Catalogs
//! are compiled into viewr, so selecting a language never performs network or
//! background work.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const MAX_PREFERENCE_BYTES: u64 = 32;

/// A bundled user-interface language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Language {
    English,
    Spanish,
    French,
    German,
}

impl Language {
    /// Translate one cataloged English source string, with an explicit English
    /// fallback for copy that has not entered the catalog yet.
    #[must_use]
    pub(crate) fn text(self, english: &'static str) -> &'static str {
        let Some(message) = MESSAGES.iter().find(|message| message.english == english) else {
            return english;
        };
        match self {
            Self::English => message.english,
            Self::Spanish => message.spanish,
            Self::French => message.french,
            Self::German => message.german,
        }
    }
}

/// Persisted language choice. System is the privacy-preserving default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Preference {
    #[default]
    System,
    English,
    Spanish,
    French,
    German,
}

impl Preference {
    pub(crate) const ALL: [Self; 5] = [
        Self::System,
        Self::English,
        Self::Spanish,
        Self::French,
        Self::German,
    ];

    #[must_use]
    pub(crate) const fn native_name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::English => "English",
            Self::Spanish => "Español",
            Self::French => "Français",
            Self::German => "Deutsch",
        }
    }

    #[must_use]
    pub(crate) fn resolve(self) -> Language {
        match self {
            Self::System => resolve_locale(system_locale_name().as_deref()),
            Self::English => Language::English,
            Self::Spanish => Language::Spanish,
            Self::French => Language::French,
            Self::German => Language::German,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::English => "en",
            Self::Spanish => "es",
            Self::French => "fr",
            Self::German => "de",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "system" => Some(Self::System),
            "en" => Some(Self::English),
            "es" => Some(Self::Spanish),
            "fr" => Some(Self::French),
            "de" => Some(Self::German),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Load {
    Loaded(Preference),
    Missing,
    Recovered(Recovery),
}

impl Load {
    pub(crate) const fn preference(self) -> Preference {
        match self {
            Self::Loaded(preference) => preference,
            Self::Missing | Self::Recovered(_) => Preference::System,
        }
    }

    pub(crate) const fn recovery(self) -> Option<Recovery> {
        match self {
            Self::Recovered(recovery) => Some(recovery),
            Self::Loaded(_) | Self::Missing => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recovery {
    Invalid,
    Oversized,
    Unreadable,
    ConfigurationUnavailable,
}

impl Recovery {
    pub(crate) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Oversized => "oversized",
            Self::Unreadable => "unreadable",
            Self::ConfigurationUnavailable => "configuration-unavailable",
        }
    }

    pub(crate) const fn notice(self) -> &'static str {
        match self {
            Self::Invalid | Self::Oversized | Self::Unreadable | Self::ConfigurationUnavailable => {
                "Could not restore the saved language. Using System."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveError {
    ConfigurationUnavailable,
    DirectoryUnavailable,
    TemporaryFileUnavailable,
    WriteFailed,
    SyncFailed,
    ReplaceFailed,
}

impl SaveError {
    pub(crate) const fn diagnostic_name(self) -> &'static str {
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

pub(crate) const fn save_failure_message() -> &'static str {
    "Language changed for this session but could not be remembered. Check local configuration storage, then choose it again."
}

pub(crate) fn load() -> Load {
    load_from_path(preference_path().as_deref())
}

pub(crate) fn save(preference: Preference) -> Result<(), SaveError> {
    let path = preference_path().ok_or(SaveError::ConfigurationUnavailable)?;
    save_to(&path, preference)
}

fn resolve_locale(locale: Option<&str>) -> Language {
    let primary = locale
        .unwrap_or_default()
        .trim()
        .split(['-', '_', '.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match primary.as_str() {
        "es" => Language::Spanish,
        "fr" => Language::French,
        "de" => Language::German,
        _ => Language::English,
    }
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code, reason = "reads one bounded locale name from Windows")]
fn system_locale_name() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    const LOCALE_NAME_CAPACITY: usize = 85;
    let mut locale = [0_u16; LOCALE_NAME_CAPACITY];
    // SAFETY: `locale` is writable for the stated element count and the API
    // writes a terminated locale name synchronously without retaining it.
    let written = unsafe {
        GetUserDefaultLocaleName(
            locale.as_mut_ptr(),
            i32::try_from(locale.len()).unwrap_or(i32::MAX),
        )
    };
    let length = usize::try_from(written).ok()?.checked_sub(1)?;
    String::from_utf16(locale.get(..length)?).ok()
}

#[cfg(target_os = "macos")]
fn system_locale_name() -> Option<String> {
    use objc2_foundation::NSLocale;

    let locale = NSLocale::currentLocale().localeIdentifier().to_string();
    (!locale.is_empty()).then_some(locale)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_locale_name() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn system_locale_name() -> Option<String> {
    None
}

fn load_from_path(path: Option<&Path>) -> Load {
    path.map_or(
        Load::Recovered(Recovery::ConfigurationUnavailable),
        load_from,
    )
}

fn load_from(path: &Path) -> Load {
    let file = match crate::fs::open_file_no_atime(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Load::Missing,
        Err(_) => return Load::Recovered(Recovery::Unreadable),
    };
    let Ok(metadata) = file.metadata() else {
        return Load::Recovered(Recovery::Unreadable);
    };
    if !metadata.is_file() {
        return Load::Recovered(Recovery::Unreadable);
    }
    if metadata.len() > MAX_PREFERENCE_BYTES {
        return Load::Recovered(Recovery::Oversized);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if file
        .take(MAX_PREFERENCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Load::Recovered(Recovery::Unreadable);
    }
    if bytes.len() as u64 > MAX_PREFERENCE_BYTES {
        return Load::Recovered(Recovery::Oversized);
    }
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return Load::Recovered(Recovery::Invalid);
    };
    Preference::parse(value).map_or(Load::Recovered(Recovery::Invalid), Load::Loaded)
}

fn save_to(path: &Path, preference: Preference) -> Result<(), SaveError> {
    let parent = path.parent().ok_or(SaveError::ConfigurationUnavailable)?;
    fs::create_dir_all(parent).map_err(|_| SaveError::DirectoryUnavailable)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| SaveError::TemporaryFileUnavailable)?;
    temporary
        .write_all(format!("{}\n", preference.as_str()).as_bytes())
        .map_err(|_| SaveError::WriteFailed)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| SaveError::SyncFailed)?;
    temporary
        .persist(path)
        .map_err(|_| SaveError::ReplaceFailed)?;
    Ok(())
}

fn preference_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join("viewr").join("language"));
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
                    .join("language")
            });
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Some(path.join("viewr").join("language"));
        }
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("viewr").join("language"));
    }
    #[allow(unreachable_code)]
    None
}

struct Message {
    english: &'static str,
    spanish: &'static str,
    french: &'static str,
    german: &'static str,
}

const MESSAGES: &[Message] = &[
    Message {
        english: "File",
        spanish: "Archivo",
        french: "Fichier",
        german: "Datei",
    },
    Message {
        english: "Edit",
        spanish: "Editar",
        french: "Modifier",
        german: "Bearbeiten",
    },
    Message {
        english: "Tools",
        spanish: "Herramientas",
        french: "Outils",
        german: "Werkzeuge",
    },
    Message {
        english: "View",
        spanish: "Ver",
        french: "Affichage",
        german: "Ansicht",
    },
    Message {
        english: "Help",
        spanish: "Ayuda",
        french: "Aide",
        german: "Hilfe",
    },
    Message {
        english: "Open File...",
        spanish: "Abrir archivo...",
        french: "Ouvrir un fichier...",
        german: "Datei öffnen...",
    },
    Message {
        english: "Open Folder...",
        spanish: "Abrir carpeta...",
        french: "Ouvrir un dossier...",
        german: "Ordner öffnen...",
    },
    Message {
        english: "Reload File",
        spanish: "Volver a cargar",
        french: "Recharger le fichier",
        german: "Datei neu laden",
    },
    Message {
        english: "Open With...",
        spanish: "Abrir con...",
        french: "Ouvrir avec...",
        german: "Öffnen mit...",
    },
    Message {
        english: "Save As...",
        spanish: "Guardar como...",
        french: "Enregistrer sous...",
        german: "Speichern unter...",
    },
    Message {
        english: "Preferences...",
        spanish: "Preferencias...",
        french: "Préférences...",
        german: "Einstellungen...",
    },
    Message {
        english: "Default Image Viewer...",
        spanish: "Visor de imágenes predeterminado...",
        french: "Visionneuse d’images par défaut...",
        german: "Standard-Bildanzeige...",
    },
    Message {
        english: "Move to Trash",
        spanish: "Mover a la papelera",
        french: "Mettre à la corbeille",
        german: "In den Papierkorb verschieben",
    },
    Message {
        english: "Permanently Delete...",
        spanish: "Eliminar permanentemente...",
        french: "Supprimer définitivement...",
        german: "Endgültig löschen...",
    },
    Message {
        english: "Full-Image Collage",
        spanish: "Collage de imágenes completas",
        french: "Mosaïque d’images complètes",
        german: "Vollbild-Collage",
    },
    Message {
        english: "Panels",
        spanish: "Paneles",
        french: "Panneaux",
        german: "Bereiche",
    },
    Message {
        english: "Panel Position",
        spanish: "Posición de paneles",
        french: "Position des panneaux",
        german: "Bereichsposition",
    },
    Message {
        english: "Image Background",
        spanish: "Fondo de imagen",
        french: "Arrière-plan de l’image",
        german: "Bildhintergrund",
    },
    Message {
        english: "Get latest release...",
        spanish: "Obtener la última versión...",
        french: "Obtenir la dernière version...",
        german: "Neueste Version abrufen...",
    },
    Message {
        english: "Get latest release",
        spanish: "Obtener la última versión",
        french: "Obtenir la dernière version",
        german: "Neueste Version abrufen",
    },
    Message {
        english: "About viewr",
        spanish: "Acerca de viewr",
        french: "À propos de viewr",
        german: "Über viewr",
    },
    Message {
        english: "Preferences",
        spanish: "Preferencias",
        french: "Préférences",
        german: "Einstellungen",
    },
    Message {
        english: "Language",
        spanish: "Idioma",
        french: "Langue",
        german: "Sprache",
    },
    Message {
        english: "Follow the operating-system language, or choose a language for viewr.",
        spanish: "Usa el idioma del sistema operativo o elige un idioma para viewr.",
        french: "Utilisez la langue du système ou choisissez une langue pour viewr.",
        german: "Die Systemsprache verwenden oder eine Sprache für viewr auswählen.",
    },
    Message {
        english: "Default folder sort",
        spanish: "Orden predeterminado de carpetas",
        french: "Tri par défaut des dossiers",
        german: "Standardordnersortierung",
    },
    Message {
        english: "Default image viewer",
        spanish: "Visor de imágenes predeterminado",
        french: "Visionneuse d’images par défaut",
        german: "Standard-Bildanzeige",
    },
    Message {
        english: "File associations remain opt in and are selected per image type.",
        spanish: "Las asociaciones de archivos son opcionales y se eligen por tipo de imagen.",
        french: "Les associations de fichiers restent facultatives et se choisissent par type d’image.",
        german: "Dateizuordnungen bleiben optional und werden pro Bildtyp gewählt.",
    },
    Message {
        english: "Open Default Image Viewer Guide...",
        spanish: "Abrir la guía del visor predeterminado...",
        french: "Ouvrir le guide de la visionneuse par défaut...",
        german: "Anleitung zur Standard-Bildanzeige öffnen...",
    },
    Message {
        english: "Close",
        spanish: "Cerrar",
        french: "Fermer",
        german: "Schließen",
    },
    Message {
        english: "Open File",
        spanish: "Abrir archivo",
        french: "Ouvrir un fichier",
        german: "Datei öffnen",
    },
    Message {
        english: "Open Folder",
        spanish: "Abrir carpeta",
        french: "Ouvrir un dossier",
        german: "Ordner öffnen",
    },
    Message {
        english: "Retry",
        spanish: "Reintentar",
        french: "Réessayer",
        german: "Erneut versuchen",
    },
    Message {
        english: "Local only. No cloud or viewr activity log.",
        spanish: "Solo local. Sin nube ni registro de actividad de viewr.",
        french: "Local uniquement. Aucun cloud ni journal d’activité viewr.",
        german: "Nur lokal. Keine Cloud und kein viewr-Aktivitätsprotokoll.",
    },
    Message {
        english: "Crop",
        spanish: "Recortar",
        french: "Recadrer",
        german: "Zuschneiden",
    },
    Message {
        english: "Apply",
        spanish: "Aplicar",
        french: "Appliquer",
        german: "Anwenden",
    },
    Message {
        english: "Cancel",
        spanish: "Cancelar",
        french: "Annuler",
        german: "Abbrechen",
    },
    Message {
        english: "Cancel Crop",
        spanish: "Cancelar recorte",
        french: "Annuler le recadrage",
        german: "Zuschneiden abbrechen",
    },
    Message {
        english: "Apply Crop",
        spanish: "Aplicar recorte",
        french: "Appliquer le recadrage",
        german: "Zuschnitt anwenden",
    },
    Message {
        english: "Spot Heal",
        spanish: "Corrección puntual",
        french: "Correction ponctuelle",
        german: "Bereichsreparatur",
    },
    Message {
        english: "Finish Spot Heal",
        spanish: "Finalizar corrección puntual",
        french: "Terminer la correction ponctuelle",
        german: "Bereichsreparatur beenden",
    },
    Message {
        english: "Finishing Spot Heal...",
        spanish: "Finalizando la corrección...",
        french: "Finalisation de la correction...",
        german: "Bereichsreparatur wird beendet...",
    },
    Message {
        english: "Undo Spot Heal",
        spanish: "Deshacer corrección puntual",
        french: "Annuler la correction ponctuelle",
        german: "Bereichsreparatur rückgängig",
    },
    Message {
        english: "Redo Spot Heal",
        spanish: "Rehacer corrección puntual",
        french: "Rétablir la correction ponctuelle",
        german: "Bereichsreparatur wiederholen",
    },
    Message {
        english: "Rotate Clockwise",
        spanish: "Girar a la derecha",
        french: "Pivoter à droite",
        german: "Im Uhrzeigersinn drehen",
    },
    Message {
        english: "Rotate Counterclockwise",
        spanish: "Girar a la izquierda",
        french: "Pivoter à gauche",
        german: "Gegen den Uhrzeigersinn drehen",
    },
    Message {
        english: "Flip Horizontally",
        spanish: "Voltear horizontalmente",
        french: "Retourner horizontalement",
        german: "Horizontal spiegeln",
    },
    Message {
        english: "Flip Vertically",
        spanish: "Voltear verticalmente",
        french: "Retourner verticalement",
        german: "Vertikal spiegeln",
    },
    Message {
        english: "Rotate counterclockwise (L)",
        spanish: "Girar a la izquierda (L)",
        french: "Pivoter à gauche (L)",
        german: "Gegen den Uhrzeigersinn drehen (L)",
    },
    Message {
        english: "Rotate clockwise (R)",
        spanish: "Girar a la derecha (R)",
        french: "Pivoter à droite (R)",
        german: "Im Uhrzeigersinn drehen (R)",
    },
    Message {
        english: "Flip horizontally (H)",
        spanish: "Voltear horizontalmente (H)",
        french: "Retourner horizontalement (H)",
        german: "Horizontal spiegeln (H)",
    },
    Message {
        english: "Flip vertically (V)",
        spanish: "Voltear verticalmente (V)",
        french: "Retourner verticalement (V)",
        german: "Vertikal spiegeln (V)",
    },
    Message {
        english: "Crop (C)",
        spanish: "Recortar (C)",
        french: "Recadrer (C)",
        german: "Zuschneiden (C)",
    },
    Message {
        english: "Spot heal (J)",
        spanish: "Corrección puntual (J)",
        french: "Correction ponctuelle (J)",
        german: "Bereichsreparatur (J)",
    },
    Message {
        english: "Image Information",
        spanish: "Información de la imagen",
        french: "Informations sur l’image",
        german: "Bildinformationen",
    },
    Message {
        english: "Folder Previews",
        spanish: "Vistas previas de carpeta",
        french: "Aperçus du dossier",
        german: "Ordnervorschauen",
    },
    Message {
        english: "System",
        spanish: "Sistema",
        french: "Système",
        german: "System",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_resolution_is_bounded_and_uses_primary_language_subtags() {
        assert_eq!(resolve_locale(Some("es-MX")), Language::Spanish);
        assert_eq!(resolve_locale(Some("fr_FR.UTF-8")), Language::French);
        assert_eq!(resolve_locale(Some("de-DE@euro")), Language::German);
        assert_eq!(resolve_locale(Some("ja-JP")), Language::English);
        assert_eq!(resolve_locale(None), Language::English);
    }

    #[test]
    fn preference_round_trips_and_catalogs_are_complete() {
        let workspace = crate::ephemeral::TempWorkspace::new("locale_preference").unwrap();
        let path = workspace.path().join("nested").join("language");
        assert_eq!(load_from(&path), Load::Missing);
        for preference in Preference::ALL {
            save_to(&path, preference).unwrap();
            assert_eq!(load_from(&path), Load::Loaded(preference));
        }
        std::fs::write(&path, "unknown\n").unwrap();
        assert_eq!(load_from(&path), Load::Recovered(Recovery::Invalid));
        std::fs::write(&path, "x".repeat(64)).unwrap();
        assert_eq!(load_from(&path), Load::Recovered(Recovery::Oversized));

        for message in MESSAGES {
            assert!(!message.english.is_empty());
            assert!(!message.spanish.is_empty());
            assert!(!message.french.is_empty());
            assert!(!message.german.is_empty());
        }
    }

    #[test]
    fn recovery_and_save_failures_are_path_private() {
        assert_eq!(Load::Missing.preference(), Preference::System);
        assert_eq!(Load::Missing.recovery(), None);
        assert_eq!(
            load_from_path(None),
            Load::Recovered(Recovery::ConfigurationUnavailable)
        );
        for recovery in [
            Recovery::Invalid,
            Recovery::Oversized,
            Recovery::Unreadable,
            Recovery::ConfigurationUnavailable,
        ] {
            assert!(!recovery.diagnostic_name().is_empty());
            assert!(!recovery.notice().contains(['\\', '/']));
        }
        for error in [
            SaveError::ConfigurationUnavailable,
            SaveError::DirectoryUnavailable,
            SaveError::TemporaryFileUnavailable,
            SaveError::WriteFailed,
            SaveError::SyncFailed,
            SaveError::ReplaceFailed,
        ] {
            assert!(!error.diagnostic_name().is_empty());
        }
        assert!(!save_failure_message().contains(['\\', '/']));
    }
}
