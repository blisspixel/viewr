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
    /// Test-only language that brackets every cataloged string, so a rendered
    /// interface can prove that no visible text bypassed the catalog.
    #[cfg(test)]
    Pseudo,
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
            #[cfg(test)]
            Self::Pseudo => pseudo_text(message.english),
        }
    }
}

/// Opening and closing marks of the test-only pseudo language.
#[cfg(test)]
pub(crate) const PSEUDO_MARKS: (char, char) = ('\u{27E6}', '\u{27E7}');

/// Bracketed pseudo translation, created once per source and kept for the
/// life of the test process so it can be returned as `&'static str`.
#[cfg(test)]
fn pseudo_text(english: &'static str) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.entry(english).or_insert_with(|| {
        let (open, close) = PSEUDO_MARKS;
        Box::leak(format!("{open}{english}{close}").into_boxed_str())
    })
}

/// User-visible text already resolved for one language.
///
/// Chrome outcome messages accept only this type, so an English literal or an
/// ad hoc `format!` cannot reach a toast without passing through the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Localized(String);

impl Localized {
    /// Text from a pure seam that takes the `Language` itself and proves, in its
    /// own tests, that every string it can return is cataloged.
    #[must_use]
    pub(crate) fn from_translated_seam(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

impl Language {
    /// Resolve one cataloged literal. Pass the literal through `tr!` so a
    /// missing entry fails the build.
    #[must_use]
    pub(crate) fn localize(self, english: &'static str) -> Localized {
        Localized(self.text(english).to_owned())
    }

    /// Resolve a cataloged template, then substitute each `{name}` placeholder
    /// in one pass. Substituted values are never rescanned, so a value that
    /// happens to contain braces is inserted verbatim. An unknown placeholder
    /// is left visible rather than silently dropped.
    #[must_use]
    pub(crate) fn fill(self, template: &'static str, values: &[(&str, &str)]) -> Localized {
        let translated = self.text(template);
        let mut text = String::with_capacity(translated.len());
        let mut rest = translated;
        while let Some(open) = rest.find('{') {
            text.push_str(&rest[..open]);
            let candidate = &rest[open..];
            let replacement = candidate.find('}').and_then(|close| {
                let name = &candidate[1..close];
                values
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (close, *value))
            });
            if let Some((close, value)) = replacement {
                text.push_str(value);
                rest = &candidate[close + 1..];
            } else {
                text.push('{');
                rest = &candidate[1..];
            }
        }
        text.push_str(rest);
        Localized(text)
    }
}

/// Compile-time catalog binding for one user-visible literal.
///
/// A literal that is not in `MESSAGES` fails the build, so new interface copy
/// cannot reach one language while silently staying English in the other three.
pub(crate) const fn assert_cataloged(english: &str) {
    let mut index = 0;
    while index < MESSAGES.len() {
        if same_source(MESSAGES[index].english, english) {
            return;
        }
        index += 1;
    }
    panic!("user-visible literal is missing from the language catalog");
}

const fn same_source(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// Catalog membership for copy that arrives as a value rather than a literal.
///
/// Pure copy seams enumerate their complete message set through this so their
/// own tests prove coverage that a literal check cannot see.
#[cfg(test)]
#[must_use]
pub(crate) fn is_cataloged(english: &str) -> bool {
    MESSAGES.iter().any(|message| message.english == english)
}

/// Bind one user-visible literal to its catalog entry at compile time.
macro_rules! tr {
    ($english:literal) => {{
        const _: () = $crate::locale::assert_cataloged($english);
        $english
    }};
}

pub(crate) use tr;

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
        // "Vollbild" is the German word for fullscreen, so it cannot carry
        // "full-image" here without naming a different feature.
        german: "Collage vollständiger Bilder",
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
    // First-run card and Help shortcut copy owned by `shortcuts`.
    Message {
        english: "Open an image",
        spanish: "Abrir una imagen",
        french: "Ouvrir une image",
        german: "Ein Bild öffnen",
    },
    Message {
        english: "Opening {subject}",
        spanish: "Abriendo {subject}",
        french: "Ouverture de {subject}",
        german: "{subject} wird geöffnet",
    },
    Message {
        english: "Could not open {subject}",
        spanish: "No se pudo abrir {subject}",
        french: "Impossible d’ouvrir {subject}",
        german: "{subject} konnte nicht geöffnet werden",
    },
    Message {
        english: "Retry opening {subject}",
        spanish: "Reintentar abrir {subject}",
        french: "Réessayer d’ouvrir {subject}",
        german: "{subject} erneut öffnen",
    },
    Message {
        english: "Retry opening the image",
        spanish: "Reintentar abrir la imagen",
        french: "Réessayer d’ouvrir l’image",
        german: "Bild erneut öffnen",
    },
    Message {
        english: "Opening the image",
        spanish: "Abriendo la imagen",
        french: "Ouverture de l’image",
        german: "Bild wird geöffnet",
    },
    Message {
        english: "Could not open the image",
        spanish: "No se pudo abrir la imagen",
        french: "Impossible d’ouvrir l’image",
        german: "Bild konnte nicht geöffnet werden",
    },
    Message {
        english: "Open File, Open Folder, or drop a file or folder. A dropped file also browses its folder when access allows. Open Folder selects the folder for this session.",
        spanish: "Abrir archivo, Abrir carpeta, o suelte un archivo o una carpeta. Un archivo soltado también explora su carpeta cuando el acceso lo permite. Abrir carpeta selecciona la carpeta para esta sesión.",
        french: "Ouvrir un fichier, Ouvrir un dossier, ou déposez un fichier ou un dossier. Un fichier déposé parcourt aussi son dossier lorsque l’accès le permet. Ouvrir un dossier sélectionne le dossier pour cette session.",
        german: "Datei öffnen, Ordner öffnen, oder ziehen Sie eine Datei oder einen Ordner hierher. Eine abgelegte Datei durchsucht auch ihren Ordner, sofern der Zugriff es erlaubt. Ordner öffnen wählt den Ordner für diese Sitzung.",
    },
    Message {
        english: "Decoding locally while the window stays responsive.",
        spanish: "Decodificando localmente mientras la ventana sigue respondiendo.",
        french: "Décodage local pendant que la fenêtre reste réactive.",
        german: "Lokale Dekodierung, während das Fenster reaktionsfähig bleibt.",
    },
    Message {
        english: "Open",
        spanish: "Abrir",
        french: "Ouvrir",
        german: "Öffnen",
    },
    Message {
        english: "Browse",
        spanish: "Explorar",
        french: "Parcourir",
        german: "Durchsuchen",
    },
    Message {
        english: "Open file",
        spanish: "Abrir archivo",
        french: "Ouvrir un fichier",
        german: "Datei öffnen",
    },
    Message {
        english: "Open folder",
        spanish: "Abrir carpeta",
        french: "Ouvrir un dossier",
        german: "Ordner öffnen",
    },
    Message {
        english: "Save As",
        spanish: "Guardar como",
        french: "Enregistrer sous",
        german: "Speichern unter",
    },
    Message {
        english: "Previous / next image",
        spanish: "Imagen anterior / siguiente",
        french: "Image précédente / suivante",
        german: "Vorheriges / nächstes Bild",
    },
    Message {
        english: "First / last image",
        spanish: "Primera / última imagen",
        french: "Première / dernière image",
        german: "Erstes / letztes Bild",
    },
    Message {
        english: "Previous / next image or collage group",
        spanish: "Imagen o grupo del collage anterior / siguiente",
        french: "Image ou groupe de la mosaïque précédent / suivant",
        german: "Vorheriges / nächstes Bild oder Collage-Gruppe",
    },
    Message {
        english: "Previous / next page or frame",
        spanish: "Página o fotograma anterior / siguiente",
        french: "Page ou trame précédente / suivante",
        german: "Vorherige / nächste Seite oder Einzelbild",
    },
    Message {
        english: "Reload file",
        spanish: "Volver a cargar el archivo",
        french: "Recharger le fichier",
        german: "Datei neu laden",
    },
    Message {
        english: "Fit; hold to pan",
        spanish: "Ajustar; mantener para desplazar",
        french: "Ajuster ; maintenir pour déplacer",
        german: "Einpassen; halten zum Verschieben",
    },
    Message {
        english: "Fit",
        spanish: "Ajustar",
        french: "Ajuster",
        german: "Einpassen",
    },
    Message {
        english: "Actual size",
        spanish: "Tamaño real",
        french: "Taille réelle",
        german: "Originalgröße",
    },
    Message {
        english: "Zoom",
        spanish: "Zoom",
        french: "Zoom",
        german: "Zoom",
    },
    Message {
        english: "Fullscreen",
        spanish: "Pantalla completa",
        french: "Plein écran",
        german: "Vollbild",
    },
    Message {
        english: "Full-image collage",
        spanish: "Collage de imágenes completas",
        french: "Mosaïque d’images complètes",
        german: "Collage vollständiger Bilder",
    },
    Message {
        english: "Leave tool, collage, or fullscreen",
        spanish: "Salir de la herramienta, el collage o la pantalla completa",
        french: "Quitter l’outil, la mosaïque ou le plein écran",
        german: "Werkzeug, Collage oder Vollbild verlassen",
    },
    Message {
        english: "Clear or set rating",
        spanish: "Borrar o asignar valoración",
        french: "Effacer ou définir la note",
        german: "Bewertung löschen oder setzen",
    },
    Message {
        english: "Crop / Spot Heal",
        spanish: "Recortar / Corrección puntual",
        french: "Recadrer / Correction ponctuelle",
        german: "Zuschneiden / Bereichsreparatur",
    },
    Message {
        english: "Rotate and flip",
        spanish: "Girar y voltear",
        french: "Pivoter et retourner",
        german: "Drehen und spiegeln",
    },
    Message {
        english: "Undo Trash",
        spanish: "Deshacer envío a la papelera",
        french: "Annuler la mise à la corbeille",
        german: "Verschieben in den Papierkorb rückgängig machen",
    },
    Message {
        english: "Permanently delete?",
        spanish: "¿Eliminar permanentemente?",
        french: "Supprimer définitivement ?",
        german: "Endgültig löschen?",
    },
    // Destructive-action copy composed by `curation_state`.
    Message {
        english: "Move to Trash stopped unexpectedly. The file may have moved. Review the folder and system Trash, then close and reopen viewr before trying another destructive action.",
        spanish: "El envío a la papelera se detuvo de forma inesperada. El archivo puede haberse movido. Revise la carpeta y la papelera del sistema, luego cierre y vuelva a abrir viewr antes de intentar otra acción destructiva.",
        french: "La mise à la corbeille s’est interrompue de façon inattendue. Le fichier a peut-être été déplacé. Vérifiez le dossier et la corbeille du système, puis fermez et rouvrez viewr avant toute autre action destructive.",
        german: "Das Verschieben in den Papierkorb wurde unerwartet beendet. Die Datei wurde möglicherweise verschoben. Prüfen Sie den Ordner und den Papierkorb des Systems, schließen Sie viewr und öffnen Sie es erneut, bevor Sie eine weitere löschende Aktion versuchen.",
    },
    Message {
        english: "Permanent delete stopped unexpectedly. The file may have been deleted. Review the folder, then close and reopen viewr before trying another destructive action.",
        spanish: "El borrado permanente se detuvo de forma inesperada. El archivo puede haberse eliminado. Revise la carpeta, luego cierre y vuelva a abrir viewr antes de intentar otra acción destructiva.",
        french: "La suppression définitive s’est interrompue de façon inattendue. Le fichier a peut-être été supprimé. Vérifiez le dossier, puis fermez et rouvrez viewr avant toute autre action destructive.",
        german: "Das endgültige Löschen wurde unerwartet beendet. Die Datei wurde möglicherweise gelöscht. Prüfen Sie den Ordner, schließen Sie viewr und öffnen Sie es erneut, bevor Sie eine weitere löschende Aktion versuchen.",
    },
    Message {
        english: "Trash restore stopped unexpectedly. Some files may have restored. Undo receipts were kept; review the folder and system Trash, then retry U before moving more files to Trash.",
        spanish: "La restauración desde la papelera se detuvo de forma inesperada. Puede que algunos archivos se hayan restaurado. Se conservaron los comprobantes para deshacer; revise la carpeta y la papelera del sistema, luego reintente con U antes de mover más archivos a la papelera.",
        french: "La restauration depuis la corbeille s’est interrompue de façon inattendue. Certains fichiers ont peut-être été restaurés. Les reçus d’annulation ont été conservés ; vérifiez le dossier et la corbeille du système, puis réessayez avec U avant de mettre d’autres fichiers à la corbeille.",
        german: "Die Wiederherstellung aus dem Papierkorb wurde unerwartet beendet. Einige Dateien wurden möglicherweise wiederhergestellt. Die Rückgängig-Belege wurden behalten; prüfen Sie den Ordner und den Papierkorb des Systems und versuchen Sie es erneut mit U, bevor Sie weitere Dateien in den Papierkorb verschieben.",
    },
    Message {
        english: "{count} file",
        spanish: "{count} archivo",
        french: "{count} fichier",
        german: "{count} Datei",
    },
    Message {
        english: "{count} files",
        spanish: "{count} archivos",
        french: "{count} fichiers",
        german: "{count} Dateien",
    },
    Message {
        english: "Moving {files} to Trash...",
        spanish: "Moviendo {files} a la papelera...",
        french: "Mise à la corbeille de {files}...",
        german: "{files} werden in den Papierkorb verschoben...",
    },
    Message {
        english: "Finishing move to Trash for {files} before closing...",
        spanish: "Finalizando el envío de {files} a la papelera antes de cerrar...",
        french: "Fin de la mise à la corbeille de {files} avant la fermeture...",
        german: "Verschieben von {files} in den Papierkorb wird vor dem Schließen abgeschlossen...",
    },
    Message {
        english: "Permanently deleting {files}...",
        spanish: "Eliminando permanentemente {files}...",
        french: "Suppression définitive de {files}...",
        german: "{files} werden endgültig gelöscht...",
    },
    Message {
        english: "Finishing permanent delete for {files} before closing...",
        spanish: "Finalizando el borrado permanente de {files} antes de cerrar...",
        french: "Fin de la suppression définitive de {files} avant la fermeture...",
        german: "Endgültiges Löschen von {files} wird vor dem Schließen abgeschlossen...",
    },
    Message {
        english: "Restoring {files} from Trash...",
        spanish: "Restaurando {files} desde la papelera...",
        french: "Restauration de {files} depuis la corbeille...",
        german: "{files} werden aus dem Papierkorb wiederhergestellt...",
    },
    Message {
        english: "Finishing Trash restore for {files} before closing...",
        spanish: "Finalizando la restauración de {files} desde la papelera antes de cerrar...",
        french: "Fin de la restauration de {files} depuis la corbeille avant la fermeture...",
        german: "Wiederherstellung von {files} aus dem Papierkorb wird vor dem Schließen abgeschlossen...",
    },
    Message {
        english: "This file changed after it was displayed. Reload it before moving it to Trash. Nothing was moved.",
        spanish: "Este archivo cambió después de mostrarse. Vuelva a cargarlo antes de moverlo a la papelera. No se movió nada.",
        french: "Ce fichier a changé après son affichage. Rechargez-le avant de le mettre à la corbeille. Rien n’a été déplacé.",
        german: "Diese Datei hat sich nach der Anzeige geändert. Laden Sie sie neu, bevor Sie sie in den Papierkorb verschieben. Es wurde nichts verschoben.",
    },
    Message {
        english: "This file changed after it was displayed. Reload it before deleting it. Nothing was deleted.",
        spanish: "Este archivo cambió después de mostrarse. Vuelva a cargarlo antes de eliminarlo. No se eliminó nada.",
        french: "Ce fichier a changé après son affichage. Rechargez-le avant de le supprimer. Rien n’a été supprimé.",
        german: "Diese Datei hat sich nach der Anzeige geändert. Laden Sie sie neu, bevor Sie sie löschen. Es wurde nichts gelöscht.",
    },
    Message {
        english: "This file is no longer available. Nothing was moved.",
        spanish: "Este archivo ya no está disponible. No se movió nada.",
        french: "Ce fichier n’est plus disponible. Rien n’a été déplacé.",
        german: "Diese Datei ist nicht mehr verfügbar. Es wurde nichts verschoben.",
    },
    Message {
        english: "This file is no longer available. Nothing was deleted.",
        spanish: "Este archivo ya no está disponible. No se eliminó nada.",
        french: "Ce fichier n’est plus disponible. Rien n’a été supprimé.",
        german: "Diese Datei ist nicht mehr verfügbar. Es wurde nichts gelöscht.",
    },
    Message {
        english: "This filesystem entry cannot be safely moved from the displayed source. Nothing was moved.",
        spanish: "Esta entrada del sistema de archivos no se puede mover de forma segura desde el origen mostrado. No se movió nada.",
        french: "Cet élément du système de fichiers ne peut pas être déplacé en toute sécurité depuis la source affichée. Rien n’a été déplacé.",
        german: "Dieser Dateisystemeintrag kann von der angezeigten Quelle nicht sicher verschoben werden. Es wurde nichts verschoben.",
    },
    Message {
        english: "This filesystem entry cannot be safely deleted from the displayed source. Nothing was deleted.",
        spanish: "Esta entrada del sistema de archivos no se puede eliminar de forma segura desde el origen mostrado. No se eliminó nada.",
        french: "Cet élément du système de fichiers ne peut pas être supprimé en toute sécurité depuis la source affichée. Rien n’a été supprimé.",
        german: "Dieser Dateisystemeintrag kann von der angezeigten Quelle nicht sicher gelöscht werden. Es wurde nichts gelöscht.",
    },
    Message {
        english: "Safe file identity could not be verified. Nothing was moved.",
        spanish: "No se pudo verificar la identidad segura del archivo. No se movió nada.",
        french: "L’identité sûre du fichier n’a pas pu être vérifiée. Rien n’a été déplacé.",
        german: "Die sichere Identität der Datei konnte nicht überprüft werden. Es wurde nichts verschoben.",
    },
    Message {
        english: "Safe file identity could not be verified. Nothing was deleted.",
        spanish: "No se pudo verificar la identidad segura del archivo. No se eliminó nada.",
        french: "L’identité sûre du fichier n’a pas pu être vérifiée. Rien n’a été supprimé.",
        german: "Die sichere Identität der Datei konnte nicht überprüft werden. Es wurde nichts gelöscht.",
    },
    Message {
        english: "Trash failed: {error}. Nothing was moved.",
        spanish: "Error al mover a la papelera: {error}. No se movió nada.",
        french: "Échec de la mise à la corbeille : {error}. Rien n’a été déplacé.",
        german: "Verschieben in den Papierkorb fehlgeschlagen: {error}. Es wurde nichts verschoben.",
    },
    Message {
        english: "Delete failed: {error}. Nothing was deleted.",
        spanish: "Error al eliminar: {error}. No se eliminó nada.",
        french: "Échec de la suppression : {error}. Rien n’a été supprimé.",
        german: "Löschen fehlgeschlagen: {error}. Es wurde nichts gelöscht.",
    },
    Message {
        english: "Moved to Trash. Undo with U.",
        spanish: "Movido a la papelera. Deshacer con U.",
        french: "Mis à la corbeille. Annuler avec U.",
        german: "In den Papierkorb verschoben. Rückgängig mit U.",
    },
    Message {
        english: "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action.",
        spanish: "Movido a la papelera, pero U no está disponible para este movimiento. Use la papelera del sistema; U todavía restaura la acción anterior.",
        french: "Mis à la corbeille, mais U n’est pas disponible pour cette opération. Utilisez la corbeille du système ; U restaure encore l’action précédente.",
        german: "In den Papierkorb verschoben, aber U ist für diesen Vorgang nicht verfügbar. Verwenden Sie den Papierkorb des Systems; U stellt weiterhin die vorherige Aktion wieder her.",
    },
    Message {
        english: "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery.",
        spanish: "Movido a la papelera, pero U no está disponible para este movimiento. Use la papelera del sistema para recuperarlo.",
        french: "Mis à la corbeille, mais U n’est pas disponible pour cette opération. Utilisez la corbeille du système pour le récupérer.",
        german: "In den Papierkorb verschoben, aber U ist für diesen Vorgang nicht verfügbar. Verwenden Sie zum Wiederherstellen den Papierkorb des Systems.",
    },
    Message {
        english: "Delete permanently",
        spanish: "Eliminar permanentemente",
        french: "Supprimer définitivement",
        german: "Endgültig löschen",
    },
    Message {
        english: "Delete \"{name}\" forever?\n\nThis skips the system Trash and cannot be undone from viewr.",
        spanish: "¿Eliminar \"{name}\" para siempre?\n\nEsto omite la papelera del sistema y no se puede deshacer desde viewr.",
        french: "Supprimer \"{name}\" définitivement ?\n\nCette action ignore la corbeille du système et ne peut pas être annulée depuis viewr.",
        german: "\"{name}\" endgültig löschen?\n\nDies umgeht den Papierkorb des Systems und kann in viewr nicht rückgängig gemacht werden.",
    },
    Message {
        english: "Permanently deleted \"{name}\". This cannot be undone; U still restores the previous Trash action.",
        spanish: "Se eliminó permanentemente \"{name}\". Esto no se puede deshacer; U todavía restaura la acción anterior de la papelera.",
        french: "\"{name}\" a été supprimé définitivement. Cette action est irréversible ; U restaure encore l’action précédente de la corbeille.",
        german: "\"{name}\" wurde endgültig gelöscht. Dies kann nicht rückgängig gemacht werden; U stellt weiterhin die vorherige Papierkorb-Aktion wieder her.",
    },
    Message {
        english: "Permanently deleted \"{name}\". This cannot be undone.",
        spanish: "Se eliminó permanentemente \"{name}\". Esto no se puede deshacer.",
        french: "\"{name}\" a été supprimé définitivement. Cette action est irréversible.",
        german: "\"{name}\" wurde endgültig gelöscht. Dies kann nicht rückgängig gemacht werden.",
    },
    Message {
        english: "Wait for this photo to finish opening before moving it to Trash",
        spanish: "Espere a que esta foto termine de abrirse antes de moverla a la papelera",
        french: "Attendez la fin de l’ouverture de cette photo avant de la mettre à la corbeille",
        german: "Warten Sie, bis dieses Foto geöffnet ist, bevor Sie es in den Papierkorb verschieben",
    },
    Message {
        english: "Wait for this photo to finish opening before permanently deleting it",
        spanish: "Espere a que esta foto termine de abrirse antes de eliminarla permanentemente",
        french: "Attendez la fin de l’ouverture de cette photo avant de la supprimer définitivement",
        german: "Warten Sie, bis dieses Foto geöffnet ist, bevor Sie es endgültig löschen",
    },
    Message {
        english: "Reload or open another image before moving it to Trash",
        spanish: "Vuelva a cargar u abra otra imagen antes de moverla a la papelera",
        french: "Rechargez ou ouvrez une autre image avant de la mettre à la corbeille",
        german: "Laden Sie das Bild neu oder öffnen Sie ein anderes, bevor Sie es in den Papierkorb verschieben",
    },
    Message {
        english: "Reload or open another image before permanently deleting it",
        spanish: "Vuelva a cargar u abra otra imagen antes de eliminarla permanentemente",
        french: "Rechargez ou ouvrez une autre image avant de la supprimer définitivement",
        german: "Laden Sie das Bild neu oder öffnen Sie ein anderes, bevor Sie es endgültig löschen",
    },
    Message {
        english: "Wait for the selected image to finish opening before moving it to Trash",
        spanish: "Espere a que la imagen seleccionada termine de abrirse antes de moverla a la papelera",
        french: "Attendez la fin de l’ouverture de l’image sélectionnée avant de la mettre à la corbeille",
        german: "Warten Sie, bis das ausgewählte Bild geöffnet ist, bevor Sie es in den Papierkorb verschieben",
    },
    Message {
        english: "Wait for the selected image to finish opening before permanently deleting it",
        spanish: "Espere a que la imagen seleccionada termine de abrirse antes de eliminarla permanentemente",
        french: "Attendez la fin de l’ouverture de l’image sélectionnée avant de la supprimer définitivement",
        german: "Warten Sie, bis das ausgewählte Bild geöffnet ist, bevor Sie es endgültig löschen",
    },
    Message {
        english: "Restore blocked: The original folder already contains an item with that name. Move or rename it, then retry with U.",
        spanish: "Restauración bloqueada: la carpeta original ya contiene un elemento con ese nombre. Muévalo o renómbrelo, luego reintente con U.",
        french: "Restauration bloquée : le dossier d’origine contient déjà un élément portant ce nom. Déplacez-le ou renommez-le, puis réessayez avec U.",
        german: "Wiederherstellung blockiert: Der ursprüngliche Ordner enthält bereits ein Element mit diesem Namen. Verschieben oder benennen Sie es um und versuchen Sie es erneut mit U.",
    },
    Message {
        english: "Restore blocked: Access was denied. Check permissions, then retry with U.",
        spanish: "Restauración bloqueada: se denegó el acceso. Compruebe los permisos, luego reintente con U.",
        french: "Restauration bloquée : l’accès a été refusé. Vérifiez les autorisations, puis réessayez avec U.",
        german: "Wiederherstellung blockiert: Der Zugriff wurde verweigert. Prüfen Sie die Berechtigungen und versuchen Sie es erneut mit U.",
    },
    Message {
        english: "Restore failed: The operating system could not restore the file. Retry with U.",
        spanish: "Error de restauración: el sistema operativo no pudo restaurar el archivo. Reintente con U.",
        french: "Échec de la restauration : le système d’exploitation n’a pas pu restaurer le fichier. Réessayez avec U.",
        german: "Wiederherstellung fehlgeschlagen: Das Betriebssystem konnte die Datei nicht wiederherstellen. Versuchen Sie es erneut mit U.",
    },
    Message {
        english: "The exact item is no longer in the system Trash. No retry remains in viewr.",
        spanish: "El elemento exacto ya no está en la papelera del sistema. No queda ningún reintento en viewr.",
        french: "L’élément exact n’est plus dans la corbeille du système. Aucune nouvelle tentative n’est possible dans viewr.",
        german: "Das genaue Element befindet sich nicht mehr im Papierkorb des Systems. In viewr ist kein weiterer Versuch möglich.",
    },
    Message {
        english: "The exact Trash receipt is ambiguous. Use the system Trash; no retry remains in viewr.",
        spanish: "El comprobante exacto de la papelera es ambiguo. Use la papelera del sistema; no queda ningún reintento en viewr.",
        french: "Le reçu exact de la corbeille est ambigu. Utilisez la corbeille du système ; aucune nouvelle tentative n’est possible dans viewr.",
        german: "Der genaue Papierkorb-Beleg ist mehrdeutig. Verwenden Sie den Papierkorb des Systems; in viewr ist kein weiterer Versuch möglich.",
    },
    Message {
        english: "In-app restore is unsupported on this platform. Use the system Trash; no retry remains in viewr.",
        spanish: "La restauración dentro de la aplicación no es compatible con esta plataforma. Use la papelera del sistema; no queda ningún reintento en viewr.",
        french: "La restauration dans l’application n’est pas prise en charge sur cette plateforme. Utilisez la corbeille du système ; aucune nouvelle tentative n’est possible dans viewr.",
        german: "Die Wiederherstellung in der App wird auf dieser Plattform nicht unterstützt. Verwenden Sie den Papierkorb des Systems; in viewr ist kein weiterer Versuch möglich.",
    },
    Message {
        english: "The exact Trash receipt is unavailable. Use the system Trash; no retry remains in viewr.",
        spanish: "El comprobante exacto de la papelera no está disponible. Use la papelera del sistema; no queda ningún reintento en viewr.",
        french: "Le reçu exact de la corbeille n’est pas disponible. Utilisez la corbeille du système ; aucune nouvelle tentative n’est possible dans viewr.",
        german: "Der genaue Papierkorb-Beleg ist nicht verfügbar. Verwenden Sie den Papierkorb des Systems; in viewr ist kein weiterer Versuch möglich.",
    },
    Message {
        english: "Restore failed. No retry remains in viewr.",
        spanish: "Error de restauración. No queda ningún reintento en viewr.",
        french: "Échec de la restauration. Aucune nouvelle tentative n’est possible dans viewr.",
        german: "Wiederherstellung fehlgeschlagen. In viewr ist kein weiterer Versuch möglich.",
    },
    Message {
        english: "Restored {files}",
        spanish: "Se restauraron {files}",
        french: "{files} restauré(s)",
        german: "{files} wiederhergestellt",
    },
    Message {
        english: "Nothing restored",
        spanish: "No se restauró nada",
        french: "Rien n’a été restauré",
        german: "Nichts wiederhergestellt",
    },
    Message {
        english: "reopen the source folder to refresh its view",
        spanish: "vuelva a abrir la carpeta de origen para actualizar su vista",
        french: "rouvrez le dossier source pour actualiser son affichage",
        german: "öffnen Sie den Quellordner erneut, um die Ansicht zu aktualisieren",
    },
    Message {
        english: "{files} can retry with U",
        spanish: "{files} se pueden reintentar con U",
        french: "{files} peuvent être réessayés avec U",
        german: "{files} können mit U erneut versucht werden",
    },
    Message {
        english: "{files} needs the blocking condition resolved, then U can retry",
        spanish: "{files} necesita que se resuelva la condición bloqueante, luego U puede reintentar",
        french: "{files} nécessite la résolution de la condition bloquante, puis U peut réessayer",
        german: "Bei {files} muss die blockierende Bedingung behoben werden, dann kann U es erneut versuchen",
    },
    Message {
        english: "{files} need the blocking condition resolved, then U can retry",
        spanish: "{files} necesitan que se resuelva la condición bloqueante, luego U puede reintentar",
        french: "{files} nécessitent la résolution de la condition bloquante, puis U peut réessayer",
        german: "Bei {files} müssen die blockierenden Bedingungen behoben werden, dann kann U es erneut versuchen",
    },
    Message {
        english: "{files} requires system Trash review",
        spanish: "{files} requiere revisión en la papelera del sistema",
        french: "{files} nécessite une vérification dans la corbeille du système",
        german: "{files} erfordert eine Prüfung im Papierkorb des Systems",
    },
    Message {
        english: "{files} require system Trash review",
        spanish: "{files} requieren revisión en la papelera del sistema",
        french: "{files} nécessitent une vérification dans la corbeille du système",
        german: "{files} erfordern eine Prüfung im Papierkorb des Systems",
    },
    Message {
        english: "{files} is no longer available for in-app restore",
        spanish: "{files} ya no está disponible para restaurar dentro de la aplicación",
        french: "{files} n’est plus disponible pour une restauration dans l’application",
        german: "{files} steht für die Wiederherstellung in der App nicht mehr zur Verfügung",
    },
    Message {
        english: "{files} are no longer available for in-app restore",
        spanish: "{files} ya no están disponibles para restaurar dentro de la aplicación",
        french: "{files} ne sont plus disponibles pour une restauration dans l’application",
        german: "{files} stehen für die Wiederherstellung in der App nicht mehr zur Verfügung",
    },
    // Concurrent-work wait copy composed by `current_work`.
    Message {
        english: "Wait for {work} to finish {action}",
        spanish: "Espere a que {work} termine {action}",
        french: "Attendez la fin de {work} {action}",
        german: "Warten Sie, bis {work} abgeschlossen ist, {action}",
    },
    Message {
        english: "the move to Trash",
        spanish: "el envío a la papelera",
        french: "la mise à la corbeille",
        german: "das Verschieben in den Papierkorb",
    },
    Message {
        english: "the permanent delete",
        spanish: "el borrado permanente",
        french: "la suppression définitive",
        german: "das endgültige Löschen",
    },
    Message {
        english: "the Trash restore",
        spanish: "la restauración desde la papelera",
        french: "la restauration depuis la corbeille",
        german: "die Wiederherstellung aus dem Papierkorb",
    },
    Message {
        english: "source verification",
        spanish: "la verificación del origen",
        french: "la vérification de la source",
        german: "die Überprüfung der Quelle",
    },
    Message {
        english: "the folder scan",
        spanish: "el escaneo de la carpeta",
        french: "l’analyse du dossier",
        german: "die Ordnerprüfung",
    },
    Message {
        english: "image preparation",
        spanish: "la preparación de la imagen",
        french: "la préparation de l’image",
        german: "die Bildvorbereitung",
    },
    Message {
        english: "the crop",
        spanish: "el recorte",
        french: "le recadrage",
        german: "das Zuschneiden",
    },
    Message {
        english: "the rating update",
        spanish: "la actualización de la valoración",
        french: "la mise à jour de la note",
        german: "die Aktualisierung der Bewertung",
    },
    Message {
        english: "before applying the crop",
        spanish: "antes de aplicar el recorte",
        french: "avant d’appliquer le recadrage",
        german: "bevor Sie den Zuschnitt anwenden",
    },
    Message {
        english: "before browsing to another image",
        spanish: "antes de pasar a otra imagen",
        french: "avant de passer à une autre image",
        german: "bevor Sie zu einem anderen Bild wechseln",
    },
    Message {
        english: "before changing Crop",
        spanish: "antes de cambiar el recorte",
        french: "avant de modifier le recadrage",
        german: "bevor Sie den Zuschnitt ändern",
    },
    Message {
        english: "before changing the rating",
        spanish: "antes de cambiar la valoración",
        french: "avant de modifier la note",
        german: "bevor Sie die Bewertung ändern",
    },
    Message {
        english: "before changing the rating filter",
        spanish: "antes de cambiar el filtro de valoración",
        french: "avant de modifier le filtre de notes",
        german: "bevor Sie den Bewertungsfilter ändern",
    },
    Message {
        english: "before changing Spot Heal",
        spanish: "antes de cambiar la corrección puntual",
        french: "avant de modifier la correction ponctuelle",
        german: "bevor Sie die Bereichsreparatur ändern",
    },
    Message {
        english: "before flipping the image",
        spanish: "antes de voltear la imagen",
        french: "avant de retourner l’image",
        german: "bevor Sie das Bild spiegeln",
    },
    Message {
        english: "before opening another folder",
        spanish: "antes de abrir otra carpeta",
        french: "avant d’ouvrir un autre dossier",
        german: "bevor Sie einen anderen Ordner öffnen",
    },
    Message {
        english: "before opening another image",
        spanish: "antes de abrir otra imagen",
        french: "avant d’ouvrir une autre image",
        german: "bevor Sie ein anderes Bild öffnen",
    },
    Message {
        english: "before opening the source in another app",
        spanish: "antes de abrir el origen en otra aplicación",
        french: "avant d’ouvrir la source dans une autre application",
        german: "bevor Sie die Quelle in einer anderen App öffnen",
    },
    Message {
        english: "before permanently deleting this file",
        spanish: "antes de eliminar permanentemente este archivo",
        french: "avant de supprimer définitivement ce fichier",
        german: "bevor Sie diese Datei endgültig löschen",
    },
    Message {
        english: "before redoing an edit",
        spanish: "antes de rehacer una edición",
        french: "avant de rétablir une modification",
        german: "bevor Sie eine Bearbeitung wiederherstellen",
    },
    Message {
        english: "before refreshing the heal source",
        spanish: "antes de actualizar el origen de la corrección",
        french: "avant d’actualiser la source de correction",
        german: "bevor Sie die Reparaturquelle aktualisieren",
    },
    Message {
        english: "before reloading this file",
        spanish: "antes de volver a cargar este archivo",
        french: "avant de recharger ce fichier",
        german: "bevor Sie diese Datei neu laden",
    },
    Message {
        english: "before restoring files from Trash",
        spanish: "antes de restaurar archivos desde la papelera",
        french: "avant de restaurer des fichiers depuis la corbeille",
        german: "bevor Sie Dateien aus dem Papierkorb wiederherstellen",
    },
    Message {
        english: "before retrying the image load",
        spanish: "antes de reintentar la carga de la imagen",
        french: "avant de réessayer le chargement de l’image",
        german: "bevor Sie das Laden des Bildes erneut versuchen",
    },
    Message {
        english: "before rotating the image",
        spanish: "antes de girar la imagen",
        french: "avant de pivoter l’image",
        german: "bevor Sie das Bild drehen",
    },
    Message {
        english: "before saving a copy",
        spanish: "antes de guardar una copia",
        french: "avant d’enregistrer une copie",
        german: "bevor Sie eine Kopie speichern",
    },
    Message {
        english: "before starting a spot-heal stroke",
        spanish: "antes de iniciar un trazo de corrección puntual",
        french: "avant de commencer un trait de correction ponctuelle",
        german: "bevor Sie einen Reparaturstrich beginnen",
    },
    Message {
        english: "before moving this file to Trash",
        spanish: "antes de mover este archivo a la papelera",
        french: "avant de mettre ce fichier à la corbeille",
        german: "bevor Sie diese Datei in den Papierkorb verschieben",
    },
    Message {
        english: "before undoing an edit",
        spanish: "antes de deshacer una edición",
        french: "avant d’annuler une modification",
        german: "bevor Sie eine Bearbeitung rückgängig machen",
    },
    Message {
        english: "Wait for the image to finish opening before using Spot Heal",
        spanish: "Espere a que la imagen termine de abrirse antes de usar la corrección puntual",
        french: "Attendez la fin de l’ouverture de l’image avant d’utiliser la correction ponctuelle",
        german: "Warten Sie, bis das Bild geöffnet ist, bevor Sie die Bereichsreparatur verwenden",
    },
    Message {
        english: "Retry the failed image load before using Spot Heal",
        spanish: "Reintente la carga fallida de la imagen antes de usar la corrección puntual",
        french: "Réessayez le chargement de l’image avant d’utiliser la correction ponctuelle",
        german: "Versuchen Sie das fehlgeschlagene Laden erneut, bevor Sie die Bereichsreparatur verwenden",
    },
    Message {
        english: "Nothing to restore from Trash",
        spanish: "No hay nada que restaurar desde la papelera",
        french: "Rien à restaurer depuis la corbeille",
        german: "Nichts aus dem Papierkorb wiederherzustellen",
    },
    // Rating write and auxiliary-loss copy owned by `rating_state`.
    Message {
        english: "Image details, animation, and rating reading stopped unexpectedly. Close and reopen viewr before continuing.",
        spanish: "La lectura de detalles, animación y valoración de la imagen se detuvo de forma inesperada. Cierre y vuelva a abrir viewr antes de continuar.",
        french: "La lecture des détails, de l’animation et de la note de l’image s’est interrompue de façon inattendue. Fermez et rouvrez viewr avant de continuer.",
        german: "Das Lesen von Bilddetails, Animation und Bewertung wurde unerwartet beendet. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie fortfahren.",
    },
    Message {
        english: "This image's rating is read-only in viewr. The file was not changed.",
        spanish: "La valoración de esta imagen es de solo lectura en viewr. El archivo no se modificó.",
        french: "La note de cette image est en lecture seule dans viewr. Le fichier n’a pas été modifié.",
        german: "Die Bewertung dieses Bildes ist in viewr schreibgeschützt. Die Datei wurde nicht geändert.",
    },
    Message {
        english: "This image has unsupported rating metadata. The file was not changed.",
        spanish: "Esta imagen tiene metadatos de valoración no compatibles. El archivo no se modificó.",
        french: "Cette image contient des métadonnées de note non prises en charge. Le fichier n’a pas été modifié.",
        german: "Dieses Bild enthält nicht unterstützte Bewertungsmetadaten. Die Datei wurde nicht geändert.",
    },
    Message {
        english: "viewr could not read this image's rating safely. The file was not changed.",
        spanish: "viewr no pudo leer de forma segura la valoración de esta imagen. El archivo no se modificó.",
        french: "viewr n’a pas pu lire la note de cette image en toute sécurité. Le fichier n’a pas été modifié.",
        german: "viewr konnte die Bewertung dieses Bildes nicht sicher lesen. Die Datei wurde nicht geändert.",
    },
    Message {
        english: "The image changed on disk before the rating could be saved. Press F5 to reload, then try again.",
        spanish: "La imagen cambió en el disco antes de que se pudiera guardar la valoración. Pulse F5 para volver a cargarla y vuelva a intentarlo.",
        french: "L’image a changé sur le disque avant l’enregistrement de la note. Appuyez sur F5 pour la recharger, puis réessayez.",
        german: "Das Bild hat sich auf dem Datenträger geändert, bevor die Bewertung gespeichert werden konnte. Drücken Sie F5 zum Neuladen und versuchen Sie es erneut.",
    },
    Message {
        english: "Could not save the rating because the image or its folder is read-only. The previous rating is unchanged.",
        spanish: "No se pudo guardar la valoración porque la imagen o su carpeta es de solo lectura. La valoración anterior no cambió.",
        french: "Impossible d’enregistrer la note car l’image ou son dossier est en lecture seule. La note précédente est inchangée.",
        german: "Die Bewertung konnte nicht gespeichert werden, weil das Bild oder sein Ordner schreibgeschützt ist. Die vorherige Bewertung bleibt unverändert.",
    },
    Message {
        english: "Could not save the rating safely. The previous rating is unchanged.",
        spanish: "No se pudo guardar la valoración de forma segura. La valoración anterior no cambió.",
        french: "Impossible d’enregistrer la note en toute sécurité. La note précédente est inchangée.",
        german: "Die Bewertung konnte nicht sicher gespeichert werden. Die vorherige Bewertung bleibt unverändert.",
    },
    Message {
        english: "The rating update could not be verified. The original image was restored.",
        spanish: "No se pudo verificar la actualización de la valoración. Se restauró la imagen original.",
        french: "La mise à jour de la note n’a pas pu être vérifiée. L’image d’origine a été restaurée.",
        german: "Die Aktualisierung der Bewertung konnte nicht überprüft werden. Das ursprüngliche Bild wurde wiederhergestellt.",
    },
    Message {
        english: "The rating update could not be verified or restored. Stop editing this image and restore it from a trusted backup.",
        spanish: "No se pudo verificar ni restaurar la actualización de la valoración. Deje de editar esta imagen y restáurela desde una copia de seguridad de confianza.",
        french: "La mise à jour de la note n’a pas pu être vérifiée ni annulée. Cessez de modifier cette image et restaurez-la depuis une sauvegarde fiable.",
        german: "Die Aktualisierung der Bewertung konnte weder überprüft noch zurückgenommen werden. Bearbeiten Sie dieses Bild nicht weiter und stellen Sie es aus einer vertrauenswürdigen Sicherung wieder her.",
    },
    // Dock, rating, and Undo Trash copy projected by `chrome`.
    Message {
        english: "Collapse tools panel",
        spanish: "Contraer el panel de herramientas",
        french: "Réduire le panneau d’outils",
        german: "Werkzeugbereich einklappen",
    },
    Message {
        english: "Expand tools panel",
        spanish: "Expandir el panel de herramientas",
        french: "Développer le panneau d’outils",
        german: "Werkzeugbereich ausklappen",
    },
    Message {
        english: "Collapse folder previews",
        spanish: "Contraer las vistas previas de la carpeta",
        french: "Réduire les aperçus du dossier",
        german: "Ordnervorschauen einklappen",
    },
    Message {
        english: "Expand folder previews",
        spanish: "Expandir las vistas previas de la carpeta",
        french: "Développer les aperçus du dossier",
        german: "Ordnervorschauen ausklappen",
    },
    Message {
        english: "Left",
        spanish: "Izquierda",
        french: "Gauche",
        german: "Links",
    },
    Message {
        english: "Right",
        spanish: "Derecha",
        french: "Droite",
        german: "Rechts",
    },
    Message {
        english: "{panel}: {side}",
        spanish: "{panel}: {side}",
        french: "{panel} : {side}",
        german: "{panel}: {side}",
    },
    Message {
        english: "Theme Default",
        spanish: "Predeterminado del tema",
        french: "Valeur par défaut du thème",
        german: "Themenstandard",
    },
    Message {
        english: "Black",
        spanish: "Negro",
        french: "Noir",
        german: "Schwarz",
    },
    Message {
        english: "Neutral Gray",
        spanish: "Gris neutro",
        french: "Gris neutre",
        german: "Neutralgrau",
    },
    Message {
        english: "White",
        spanish: "Blanco",
        french: "Blanc",
        german: "Weiß",
    },
    Message {
        english: "{label}. {help}",
        spanish: "{label}. {help}",
        french: "{label}. {help}",
        german: "{label}. {help}",
    },
    Message {
        english: "Trash restore state is not settled. Follow the current status or recovery guidance before using Undo Trash.",
        spanish: "El estado de la restauración desde la papelera no está resuelto. Siga el estado actual o la guía de recuperación antes de deshacer la papelera.",
        french: "L’état de la restauration depuis la corbeille n’est pas stabilisé. Suivez le statut actuel ou les conseils de récupération avant d’annuler la corbeille.",
        german: "Der Zustand der Papierkorb-Wiederherstellung ist nicht geklärt. Folgen Sie dem aktuellen Status oder der Wiederherstellungsanleitung, bevor Sie den Papierkorb rückgängig machen.",
    },
    Message {
        english: "Restores the latest safely recoverable Trash action. It may belong to another folder.",
        spanish: "Restaura la acción de papelera más reciente que se puede recuperar de forma segura. Puede pertenecer a otra carpeta.",
        french: "Restaure la dernière action de corbeille récupérable en toute sécurité. Elle peut appartenir à un autre dossier.",
        german: "Stellt die letzte sicher wiederherstellbare Papierkorb-Aktion wieder her. Sie kann zu einem anderen Ordner gehören.",
    },
    Message {
        english: "No safely recoverable Trash action is available.",
        spanish: "No hay ninguna acción de papelera que se pueda recuperar de forma segura.",
        french: "Aucune action de corbeille récupérable en toute sécurité n’est disponible.",
        german: "Es ist keine sicher wiederherstellbare Papierkorb-Aktion verfügbar.",
    },
    Message {
        english: "Rating: Loading image",
        spanish: "Valoración: cargando la imagen",
        french: "Note : chargement de l’image",
        german: "Bewertung: Bild wird geladen",
    },
    Message {
        english: "Rating: Image unavailable",
        spanish: "Valoración: imagen no disponible",
        french: "Note : image indisponible",
        german: "Bewertung: Bild nicht verfügbar",
    },
    Message {
        english: "Rating: Open an image",
        spanish: "Valoración: abra una imagen",
        french: "Note : ouvrez une image",
        german: "Bewertung: Bild öffnen",
    },
    Message {
        english: "Rating: Reading...",
        spanish: "Valoración: leyendo...",
        french: "Note : lecture...",
        german: "Bewertung: wird gelesen...",
    },
    Message {
        english: "Rating: Unrated",
        spanish: "Valoración: sin valorar",
        french: "Note : non notée",
        german: "Bewertung: nicht bewertet",
    },
    Message {
        english: "Rating: JPEG only",
        spanish: "Valoración: solo JPEG",
        french: "Note : JPEG uniquement",
        german: "Bewertung: nur JPEG",
    },
    Message {
        english: "Rating: Rejected",
        spanish: "Valoración: rechazada",
        french: "Note : rejetée",
        german: "Bewertung: abgelehnt",
    },
    Message {
        english: "Rating: Conflict",
        spanish: "Valoración: conflicto",
        french: "Note : conflit",
        german: "Bewertung: Konflikt",
    },
    Message {
        english: "Rating: Unsupported",
        spanish: "Valoración: no compatible",
        french: "Note : non prise en charge",
        german: "Bewertung: nicht unterstützt",
    },
    Message {
        english: "Rating: Unreadable",
        spanish: "Valoración: ilegible",
        french: "Note : illisible",
        german: "Bewertung: nicht lesbar",
    },
    Message {
        english: "Rating: {rating} of 5",
        spanish: "Valoración: {rating} de 5",
        french: "Note : {rating} sur 5",
        german: "Bewertung: {rating} von 5",
    },
    Message {
        english: "{rating} of 5",
        spanish: "{rating} de 5",
        french: "{rating} sur 5",
        german: "{rating} von 5",
    },
    Message {
        english: "Unrated",
        spanish: "Sin valorar",
        french: "Non notée",
        german: "Nicht bewertet",
    },
    Message {
        english: "Rating {label}, shortcut {shortcut}",
        spanish: "Valoración {label}, atajo {shortcut}",
        french: "Note {label}, raccourci {shortcut}",
        german: "Bewertung {label}, Tastenkürzel {shortcut}",
    },
    Message {
        english: "All images",
        spanish: "Todas las imágenes",
        french: "Toutes les images",
        german: "Alle Bilder",
    },
    Message {
        english: "At least {rating}",
        spanish: "Al menos {rating}",
        french: "Au moins {rating}",
        german: "Mindestens {rating}",
    },
    Message {
        english: "Rating filter: {label}",
        spanish: "Filtro de valoración: {label}",
        french: "Filtre de notes : {label}",
        german: "Bewertungsfilter: {label}",
    },
    Message {
        english: "Rating Filter: All images",
        spanish: "Filtro de valoración: todas las imágenes",
        french: "Filtre de notes : toutes les images",
        german: "Bewertungsfilter: alle Bilder",
    },
    Message {
        english: "Rating Filter: At least {rating}",
        spanish: "Filtro de valoración: al menos {rating}",
        french: "Filtre de notes : au moins {rating}",
        german: "Bewertungsfilter: mindestens {rating}",
    },
    Message {
        english: "Rating Filter: Reading folder...",
        spanish: "Filtro de valoración: leyendo la carpeta...",
        french: "Filtre de notes : lecture du dossier...",
        german: "Bewertungsfilter: Ordner wird gelesen...",
    },
    Message {
        english: "Rating Filter: Open a folder",
        spanish: "Filtro de valoración: abra una carpeta",
        french: "Filtre de notes : ouvrez un dossier",
        german: "Bewertungsfilter: Ordner öffnen",
    },
    Message {
        english: "Wait for the selected image to finish loading.",
        spanish: "Espere a que termine de cargarse la imagen seleccionada.",
        french: "Attendez la fin du chargement de l’image sélectionnée.",
        german: "Warten Sie, bis das ausgewählte Bild geladen ist.",
    },
    Message {
        english: "Reload or open another image before assigning a rating.",
        spanish: "Vuelva a cargar u abra otra imagen antes de asignar una valoración.",
        french: "Rechargez ou ouvrez une autre image avant d’attribuer une note.",
        german: "Laden Sie das Bild neu oder öffnen Sie ein anderes, bevor Sie eine Bewertung vergeben.",
    },
    Message {
        english: "Open an image to assign a rating.",
        spanish: "Abra una imagen para asignar una valoración.",
        french: "Ouvrez une image pour attribuer une note.",
        german: "Öffnen Sie ein Bild, um eine Bewertung zu vergeben.",
    },
    Message {
        english: "Rating is not ready yet.",
        spanish: "La valoración aún no está lista.",
        french: "La note n’est pas encore prête.",
        german: "Die Bewertung ist noch nicht bereit.",
    },
    Message {
        english: "This image's rating is read-only in viewr.",
        spanish: "La valoración de esta imagen es de solo lectura en viewr.",
        french: "La note de cette image est en lecture seule dans viewr.",
        german: "Die Bewertung dieses Bildes ist in viewr schreibgeschützt.",
    },
    Message {
        english: "Safe source identity is unavailable for rating writes.",
        spanish: "No se puede confirmar de forma segura la identidad del origen para escribir la valoración.",
        french: "L’identité sûre de la source n’est pas disponible pour écrire la note.",
        german: "Die sichere Identität der Quelle steht zum Schreiben der Bewertung nicht zur Verfügung.",
    },
    Message {
        english: "Rating could not be read. Close and reopen viewr before changing it.",
        spanish: "No se pudo leer la valoración. Cierre y vuelva a abrir viewr antes de cambiarla.",
        french: "La note n’a pas pu être lue. Fermez et rouvrez viewr avant de la modifier.",
        german: "Die Bewertung konnte nicht gelesen werden. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie sie ändern.",
    },
    Message {
        english: "This image has unsupported rating metadata.",
        spanish: "Esta imagen tiene metadatos de valoración no compatibles.",
        french: "Cette image contient des métadonnées de note non prises en charge.",
        german: "Dieses Bild enthält nicht unterstützte Bewertungsmetadaten.",
    },
    Message {
        english: "Rating update is not settled. Restore this image from a trusted backup, then press F5 to reload.",
        spanish: "La actualización de la valoración no está resuelta. Restaure esta imagen desde una copia de seguridad de confianza y pulse F5 para volver a cargarla.",
        french: "La mise à jour de la note n’est pas stabilisée. Restaurez cette image depuis une sauvegarde fiable, puis appuyez sur F5 pour la recharger.",
        german: "Die Aktualisierung der Bewertung ist nicht abgeschlossen. Stellen Sie dieses Bild aus einer vertrauenswürdigen Sicherung wieder her und drücken Sie F5 zum Neuladen.",
    },
    Message {
        english: "Wait for folder ratings to finish loading before changing this rating.",
        spanish: "Espere a que terminen de cargarse las valoraciones de la carpeta antes de cambiar esta valoración.",
        french: "Attendez la fin du chargement des notes du dossier avant de modifier cette note.",
        german: "Warten Sie, bis die Bewertungen des Ordners geladen sind, bevor Sie diese Bewertung ändern.",
    },
    Message {
        english: "Save As stopped unexpectedly. Close and reopen viewr before saving again.",
        spanish: "Guardar como se detuvo de forma inesperada. Cierre y vuelva a abrir viewr antes de guardar de nuevo.",
        french: "Enregistrer sous s’est interrompu de façon inattendue. Fermez et rouvrez viewr avant d’enregistrer à nouveau.",
        german: "Speichern unter wurde unerwartet beendet. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie erneut speichern.",
    },
    Message {
        english: "Crop stopped unexpectedly. Close and reopen viewr before cropping again.",
        spanish: "El recorte se detuvo de forma inesperada. Cierre y vuelva a abrir viewr antes de recortar de nuevo.",
        french: "Le recadrage s’est interrompu de façon inattendue. Fermez et rouvrez viewr avant de recadrer à nouveau.",
        german: "Der Zuschnitt wurde unerwartet beendet. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie erneut zuschneiden.",
    },
    Message {
        english: "Display preview preparation stopped unexpectedly. Close and reopen viewr before opening another over-limit image or cropping again.",
        spanish: "La preparación de la vista previa se detuvo de forma inesperada. Cierre y vuelva a abrir viewr antes de abrir otra imagen que exceda el límite o recortar de nuevo.",
        french: "La préparation de l’aperçu s’est interrompue de façon inattendue. Fermez et rouvrez viewr avant d’ouvrir une autre image hors limite ou de recadrer à nouveau.",
        german: "Die Vorbereitung der Anzeigevorschau wurde unerwartet beendet. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie ein weiteres Bild über dem Limit öffnen oder erneut zuschneiden.",
    },
    Message {
        english: "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again.",
        spanish: "El recorte no se aplicó. La imagen original no cambió; se restauró la selección. Pulse Intro para intentarlo de nuevo.",
        french: "Le recadrage n’a pas été appliqué. L’image d’origine est inchangée ; la sélection a été restaurée. Appuyez sur Entrée pour réessayer.",
        german: "Der Zuschnitt wurde nicht angewendet. Das Originalbild ist unverändert; die Auswahl wurde wiederhergestellt. Drücken Sie die Eingabetaste, um es erneut zu versuchen.",
    },
    Message {
        english: "Crop was not applied because the image changed.",
        spanish: "El recorte no se aplicó porque la imagen cambió.",
        french: "Le recadrage n’a pas été appliqué parce que l’image a changé.",
        german: "Der Zuschnitt wurde nicht angewendet, weil sich das Bild geändert hat.",
    },
    Message {
        english: "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again.",
        spanish: "El recorte se detuvo de forma inesperada. La imagen original no cambió; se restauró la selección. Cierre y vuelva a abrir viewr antes de recortar de nuevo.",
        french: "Le recadrage s’est interrompu de façon inattendue. L’image d’origine est inchangée ; la sélection a été restaurée. Fermez et rouvrez viewr avant de recadrer à nouveau.",
        german: "Der Zuschnitt wurde unerwartet beendet. Das Originalbild ist unverändert; die Auswahl wurde wiederhergestellt. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie erneut zuschneiden.",
    },
    Message {
        english: "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again.",
        spanish: "El recorte se detuvo de forma inesperada después de que la imagen cambiara. Cierre y vuelva a abrir viewr antes de recortar de nuevo.",
        french: "Le recadrage s’est interrompu de façon inattendue après le changement de l’image. Fermez et rouvrez viewr avant de recadrer à nouveau.",
        german: "Der Zuschnitt wurde unerwartet beendet, nachdem sich das Bild geändert hat. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie erneut zuschneiden.",
    },
    Message {
        english: "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again.",
        spanish: "El recorte no pudo terminar porque la preparación de la vista previa se detuvo de forma inesperada. La imagen original no cambió; se restauró la selección. Cierre y vuelva a abrir viewr antes de recortar de nuevo.",
        french: "Le recadrage n’a pas pu se terminer parce que la préparation de l’aperçu s’est interrompue de façon inattendue. L’image d’origine est inchangée ; la sélection a été restaurée. Fermez et rouvrez viewr avant de recadrer à nouveau.",
        german: "Der Zuschnitt konnte nicht abgeschlossen werden, weil die Vorbereitung der Anzeigevorschau unerwartet beendet wurde. Das Originalbild ist unverändert; die Auswahl wurde wiederhergestellt. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie erneut zuschneiden.",
    },
    Message {
        english: "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again.",
        spanish: "La preparación de la vista previa se detuvo de forma inesperada después de que la imagen cambiara. Cierre y vuelva a abrir viewr antes de recortar de nuevo.",
        french: "La préparation de l’aperçu s’est interrompue de façon inattendue après le changement de l’image. Fermez et rouvrez viewr avant de recadrer à nouveau.",
        german: "Die Vorbereitung der Anzeigevorschau wurde unerwartet beendet, nachdem sich das Bild geändert hat. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie erneut zuschneiden.",
    },
    Message {
        english: "Wait for the image to finish opening before cropping",
        spanish: "Espere a que la imagen termine de abrirse antes de recortar",
        french: "Attendez la fin de l’ouverture de l’image avant de recadrer",
        german: "Warten Sie, bis das Bild geöffnet ist, bevor Sie zuschneiden",
    },
    Message {
        english: "Retry the failed image load before cropping",
        spanish: "Reintente la carga fallida de la imagen antes de recortar",
        french: "Réessayez le chargement de l’image avant de recadrer",
        german: "Wiederholen Sie das fehlgeschlagene Laden des Bildes, bevor Sie zuschneiden",
    },
    Message {
        english: "Wait for Spot Heal to finish before reloading",
        spanish: "Espere a que termine la corrección puntual antes de volver a cargar",
        french: "Attendez la fin de la correction ponctuelle avant de recharger",
        german: "Warten Sie, bis die Bereichsreparatur abgeschlossen ist, bevor Sie neu laden",
    },
    Message {
        english: "Wait for the crop to finish before reloading",
        spanish: "Espere a que termine el recorte antes de volver a cargar",
        french: "Attendez la fin du recadrage avant de recharger",
        german: "Warten Sie, bis der Zuschnitt abgeschlossen ist, bevor Sie neu laden",
    },
    Message {
        english: "Wait for Save As to finish before reloading",
        spanish: "Espere a que termine Guardar como antes de volver a cargar",
        french: "Attendez la fin d’Enregistrer sous avant de recharger",
        german: "Warten Sie, bis Speichern unter abgeschlossen ist, bevor Sie neu laden",
    },
    Message {
        english: "Wait for the rating update to finish before reloading",
        spanish: "Espere a que termine la actualización de la valoración antes de volver a cargar",
        french: "Attendez la fin de la mise à jour de la note avant de recharger",
        german: "Warten Sie, bis die Bewertungsaktualisierung abgeschlossen ist, bevor Sie neu laden",
    },
    Message {
        english: "Wait for folder ratings to finish loading before reloading",
        spanish: "Espere a que terminen de cargarse las valoraciones de la carpeta antes de volver a cargar",
        french: "Attendez la fin du chargement des notes du dossier avant de recharger",
        german: "Warten Sie, bis die Bewertungen des Ordners geladen sind, bevor Sie neu laden",
    },
    Message {
        english: "An image is already loading",
        spanish: "Ya se está cargando una imagen",
        french: "Une image est déjà en cours de chargement",
        german: "Ein Bild wird bereits geladen",
    },
    Message {
        english: "Source may have changed. Press F5 when it is safe to reload.",
        spanish: "El origen puede haber cambiado. Pulse F5 cuando sea seguro volver a cargar.",
        french: "La source a peut-être changé. Appuyez sur F5 lorsqu’il est sûr de recharger.",
        german: "Die Quelle hat sich möglicherweise geändert. Drücken Sie F5, wenn ein Neuladen sicher ist.",
    },
    Message {
        english: "This file is no longer at its selected path. The last good image remains visible.",
        spanish: "Este archivo ya no está en la ruta seleccionada. La última imagen válida permanece visible.",
        french: "Ce fichier n’est plus à l’emplacement sélectionné. La dernière image valide reste visible.",
        german: "Diese Datei liegt nicht mehr am ausgewählten Pfad. Das letzte gültige Bild bleibt sichtbar.",
    },
    Message {
        english: "This file was renamed. The last good image remains visible",
        spanish: "Este archivo fue renombrado. La última imagen válida permanece visible",
        french: "Ce fichier a été renommé. La dernière image valide reste visible",
        german: "Diese Datei wurde umbenannt. Das letzte gültige Bild bleibt sichtbar",
    },
    Message {
        english: "Wait for the selected folder to finish opening before saving a copy",
        spanish: "Espere a que termine de abrirse la carpeta seleccionada antes de guardar una copia",
        french: "Attendez la fin de l’ouverture du dossier sélectionné avant d’enregistrer une copie",
        german: "Warten Sie, bis der ausgewählte Ordner geöffnet ist, bevor Sie eine Kopie speichern",
    },
    Message {
        english: "Wait for the rating update to finish before saving a copy",
        spanish: "Espere a que termine la actualización de la valoración antes de guardar una copia",
        french: "Attendez la fin de la mise à jour de la note avant d’enregistrer une copie",
        german: "Warten Sie, bis die Bewertungsaktualisierung abgeschlossen ist, bevor Sie eine Kopie speichern",
    },
    Message {
        english: "Wait for the image preview to finish before saving",
        spanish: "Espere a que termine la vista previa de la imagen antes de guardar",
        french: "Attendez la fin de l’aperçu de l’image avant d’enregistrer",
        german: "Warten Sie, bis die Bildvorschau abgeschlossen ist, bevor Sie speichern",
    },
    Message {
        english: "Wait for spot heal to finish before saving",
        spanish: "Espere a que termine la corrección puntual antes de guardar",
        french: "Attendez la fin de la correction ponctuelle avant d’enregistrer",
        german: "Warten Sie, bis die Bereichsreparatur abgeschlossen ist, bevor Sie speichern",
    },
    Message {
        english: "Wait for the crop to finish before saving",
        spanish: "Espere a que termine el recorte antes de guardar",
        french: "Attendez la fin du recadrage avant d’enregistrer",
        german: "Warten Sie, bis der Zuschnitt abgeschlossen ist, bevor Sie speichern",
    },
    Message {
        english: "Apply or cancel the crop before saving a copy",
        spanish: "Aplique o cancele el recorte antes de guardar una copia",
        french: "Appliquez ou annulez le recadrage avant d’enregistrer une copie",
        german: "Wenden Sie den Zuschnitt an oder brechen Sie ihn ab, bevor Sie eine Kopie speichern",
    },
    Message {
        english: "A copy is already being saved",
        spanish: "Ya se está guardando una copia",
        french: "Une copie est déjà en cours d’enregistrement",
        german: "Eine Kopie wird bereits gespeichert",
    },
    Message {
        english: "Undo",
        spanish: "Deshacer",
        french: "Annuler",
        german: "Rückgängig",
    },
    Message {
        english: "Redo",
        spanish: "Rehacer",
        french: "Rétablir",
        german: "Wiederholen",
    },
    Message {
        english: "Spot heal",
        spanish: "Corrección puntual",
        french: "Correction ponctuelle",
        german: "Bereichsreparatur",
    },
    Message {
        english: "{action} could not be applied. The image and edit history are unchanged.",
        spanish: "No se pudo aplicar {action}. La imagen y el historial de edición no cambiaron.",
        french: "{action} n’a pas pu être appliqué. L’image et l’historique d’édition sont inchangés.",
        german: "{action} konnte nicht angewendet werden. Das Bild und der Bearbeitungsverlauf sind unverändert.",
    },
    Message {
        english: "{action} was not applied because the display could not update. Try again.",
        spanish: "No se aplicó {action} porque la pantalla no pudo actualizarse. Inténtelo de nuevo.",
        french: "{action} n’a pas été appliqué parce que l’affichage n’a pas pu se mettre à jour. Réessayez.",
        german: "{action} wurde nicht angewendet, weil die Anzeige nicht aktualisiert werden konnte. Versuchen Sie es erneut.",
    },
    Message {
        english: "{action} failed. Disk source unchanged; reloading it and clearing edit history.",
        spanish: "{action} falló. El origen en disco no cambió; se vuelve a cargar y se borra el historial de edición.",
        french: "{action} a échoué. La source sur disque est inchangée ; rechargement et effacement de l’historique d’édition.",
        german: "{action} ist fehlgeschlagen. Die Quelle auf dem Datenträger ist unverändert; sie wird neu geladen und der Bearbeitungsverlauf gelöscht.",
    },
    Message {
        english: "{action} failed. Disk source unchanged; reopen it. Edit history was cleared.",
        spanish: "{action} falló. El origen en disco no cambió; ábralo de nuevo. Se borró el historial de edición.",
        french: "{action} a échoué. La source sur disque est inchangée ; rouvrez-la. L’historique d’édition a été effacé.",
        german: "{action} ist fehlgeschlagen. Die Quelle auf dem Datenträger ist unverändert; öffnen Sie sie erneut. Der Bearbeitungsverlauf wurde gelöscht.",
    },
    Message {
        english: "Saving rating...",
        spanish: "Guardando la valoración...",
        french: "Enregistrement de la note...",
        german: "Bewertung wird gespeichert...",
    },
    Message {
        english: "Reading folder ratings...",
        spanish: "Leyendo las valoraciones de la carpeta...",
        french: "Lecture des notes du dossier...",
        german: "Ordnerbewertungen werden gelesen...",
    },
    Message {
        english: "Preparing preview...",
        spanish: "Preparando la vista previa...",
        french: "Préparation de l’aperçu...",
        german: "Vorschau wird vorbereitet...",
    },
    Message {
        english: "Saving...",
        spanish: "Guardando...",
        french: "Enregistrement...",
        german: "Speichern...",
    },
    Message {
        english: "Applying crop...",
        spanish: "Aplicando el recorte...",
        french: "Application du recadrage...",
        german: "Zuschnitt wird angewendet...",
    },
    Message {
        english: "Reading folder...",
        spanish: "Leyendo la carpeta...",
        french: "Lecture du dossier...",
        german: "Ordner wird gelesen...",
    },
    Message {
        english: "Outside current filter. Next or Previous returns to matching images.",
        spanish: "Fuera del filtro actual. Siguiente o anterior vuelve a las imágenes coincidentes.",
        french: "Hors du filtre actuel. Suivant ou précédent revient aux images correspondantes.",
        german: "Außerhalb des aktuellen Filters. Weiter oder Zurück kehrt zu passenden Bildern zurück.",
    },
    Message {
        english: "Edited pixels are in memory. Save As writes a copy.",
        spanish: "Los píxeles editados están en memoria. Guardar como escribe una copia.",
        french: "Les pixels modifiés sont en mémoire. Enregistrer sous écrit une copie.",
        german: "Bearbeitete Pixel sind im Speicher. Speichern unter schreibt eine Kopie.",
    },
    Message {
        english: "Latest First",
        spanish: "Más recientes primero",
        french: "Plus récents d’abord",
        german: "Neueste zuerst",
    },
    Message {
        english: "Name",
        spanish: "Nombre",
        french: "Nom",
        german: "Name",
    },
    Message {
        english: "Folder Sort: {sort}",
        spanish: "Orden de carpeta: {sort}",
        french: "Tri du dossier : {sort}",
        german: "Ordnersortierung: {sort}",
    },
    Message {
        english: "Default folder sort: {sort}",
        spanish: "Orden de carpeta predeterminado: {sort}",
        french: "Tri de dossier par défaut : {sort}",
        german: "Standard-Ordnersortierung: {sort}",
    },
    Message {
        english: "Source may have changed",
        spanish: "El origen puede haber cambiado",
        french: "La source a peut-être changé",
        german: "Die Quelle hat sich möglicherweise geändert",
    },
    Message {
        english: "Close panel (I)",
        spanish: "Cerrar panel (I)",
        french: "Fermer le panneau (I)",
        german: "Bedienfeld schließen (I)",
    },
    Message {
        english: "Color · {profile}",
        spanish: "Color · {profile}",
        french: "Couleur · {profile}",
        german: "Farbe · {profile}",
    },
    Message {
        english: "Display · {status}",
        spanish: "Pantalla · {status}",
        french: "Affichage · {status}",
        german: "Anzeige · {status}",
    },
    Message {
        english: "sRGB",
        spanish: "sRGB",
        french: "sRGB",
        german: "sRGB",
    },
    Message {
        english: "Embedded color metadata: sRGB",
        spanish: "Metadatos de color incrustados: sRGB",
        french: "Métadonnées de couleur intégrées : sRGB",
        german: "Eingebettete Farbmetadaten: sRGB",
    },
    Message {
        english: "Embedded ICC converted to sRGB",
        spanish: "ICC incrustado convertido a sRGB",
        french: "ICC intégré converti en sRGB",
        german: "Eingebettetes ICC nach sRGB konvertiert",
    },
    Message {
        english: "Embedded color metadata unavailable; sRGB fallback",
        spanish: "Metadatos de color incrustados no disponibles; reserva sRGB",
        french: "Métadonnées de couleur intégrées indisponibles ; repli sRGB",
        german: "Eingebettete Farbmetadaten nicht verfügbar; sRGB-Rückfall",
    },
    Message {
        english: "Worker color space unknown; sRGB fallback",
        spanish: "Espacio de color del proceso auxiliar desconocido; reserva sRGB",
        french: "Espace colorimétrique du processus auxiliaire inconnu ; repli sRGB",
        german: "Farbraum des Hilfsprozesses unbekannt; sRGB-Rückfall",
    },
    Message {
        english: "sRGB, operating-system managed",
        spanish: "sRGB, gestionado por el sistema operativo",
        french: "sRGB, géré par le système d’exploitation",
        german: "sRGB, vom Betriebssystem verwaltet",
    },
    Message {
        english: "sRGB, display profile applied",
        spanish: "sRGB, perfil de pantalla aplicado",
        french: "sRGB, profil d’affichage appliqué",
        german: "sRGB, Anzeigeprofil angewendet",
    },
    Message {
        english: "sRGB fallback",
        spanish: "Reserva sRGB",
        french: "Repli sRGB",
        german: "sRGB-Rückfall",
    },
    Message {
        english: "Capture",
        spanish: "Captura",
        french: "Prise de vue",
        german: "Aufnahme",
    },
    Message {
        english: "Camera",
        spanish: "Cámara",
        french: "Appareil photo",
        german: "Kamera",
    },
    Message {
        english: "Lens",
        spanish: "Objetivo",
        french: "Objectif",
        german: "Objektiv",
    },
    Message {
        english: "Captured",
        spanish: "Capturado",
        french: "Prise le",
        german: "Aufgenommen",
    },
    Message {
        english: "Settings",
        spanish: "Ajustes",
        french: "Réglages",
        german: "Einstellungen",
    },
    Message {
        english: "Source Privacy",
        spanish: "Privacidad del origen",
        french: "Confidentialité de la source",
        german: "Quellprivatsphäre",
    },
    Message {
        english: "Export Privacy",
        spanish: "Privacidad de exportación",
        french: "Confidentialité de l’export",
        german: "Exportprivatsphäre",
    },
    Message {
        english: "No supported EXIF detected.",
        spanish: "No se detectó EXIF compatible.",
        french: "Aucun EXIF pris en charge n’a été détecté.",
        german: "Kein unterstütztes EXIF erkannt.",
    },
    Message {
        english: "{count} supported EXIF tag detected.",
        spanish: "{count} etiqueta EXIF compatible detectada.",
        french: "{count} balise EXIF prise en charge détectée.",
        german: "{count} unterstütztes EXIF-Tag erkannt.",
    },
    Message {
        english: "{count} supported EXIF tags detected.",
        spanish: "{count} etiquetas EXIF compatibles detectadas.",
        french: "{count} balises EXIF prises en charge détectées.",
        german: "{count} unterstützte EXIF-Tags erkannt.",
    },
    Message {
        english: "No common identity or location fields detected.",
        spanish: "No se detectaron campos comunes de identidad o ubicación.",
        french: "Aucun champ d’identité ou de localisation courant n’a été détecté.",
        german: "Keine üblichen Identitäts- oder Ortsfelder erkannt.",
    },
    Message {
        english: "Present: {category}",
        spanish: "Presente: {category}",
        french: "Présent : {category}",
        german: "Vorhanden: {category}",
    },
    Message {
        english: "location-related data",
        spanish: "datos de ubicación",
        french: "données de localisation",
        german: "ortsbezogene Daten",
    },
    Message {
        english: "owner or author data",
        spanish: "datos de propietario o autor",
        french: "données de propriétaire ou d’auteur",
        german: "Eigentümer- oder Autordaten",
    },
    Message {
        english: "camera, lens, or image identifiers",
        spanish: "identificadores de cámara, objetivo o imagen",
        french: "identifiants d’appareil, d’objectif ou d’image",
        german: "Kamera-, Objektiv- oder Bildkennungen",
    },
    Message {
        english: "description or comment data",
        spanish: "datos de descripción o comentario",
        french: "données de description ou de commentaire",
        german: "Beschreibungs- oder Kommentardaten",
    },
    Message {
        english: "software history",
        spanish: "historial de software",
        french: "historique logiciel",
        german: "Softwareverlauf",
    },
    Message {
        english: "embedded thumbnail",
        spanish: "miniatura incrustada",
        french: "vignette intégrée",
        german: "eingebettetes Vorschaubild",
    },
    Message {
        english: "maker-specific data",
        spanish: "datos específicos del fabricante",
        french: "données spécifiques au fabricant",
        german: "herstellerspezifische Daten",
    },
    Message {
        english: "Presence only. Sensitive values stay hidden on screen.",
        spanish: "Solo presencia. Los valores sensibles permanecen ocultos en pantalla.",
        french: "Présence uniquement. Les valeurs sensibles restent masquées à l’écran.",
        german: "Nur Vorhandensein. Sensible Werte bleiben auf dem Bildschirm verborgen.",
    },
    Message {
        english: "Limited EXIF scan. Other metadata or hidden pixel data may still exist.",
        spanish: "Exploración EXIF limitada. Pueden existir otros metadatos o datos de píxeles ocultos.",
        french: "Analyse EXIF limitée. D’autres métadonnées ou données de pixels cachées peuvent encore exister.",
        german: "Begrenzte EXIF-Prüfung. Weitere Metadaten oder versteckte Pixeldaten können trotzdem vorhanden sein.",
    },
    Message {
        english: "Keep camera metadata when saving",
        spanish: "Conservar los metadatos de la cámara al guardar",
        french: "Conserver les métadonnées de l’appareil lors de l’enregistrement",
        german: "Kamerametadaten beim Speichern behalten",
    },
    Message {
        english: "When enabled, Save As keeps supported EXIF tags, including GPS.",
        spanish: "Si está activado, Guardar como conserva las etiquetas EXIF compatibles, incluido GPS.",
        french: "Si cette option est activée, Enregistrer sous conserve les balises EXIF prises en charge, y compris le GPS.",
        german: "Wenn aktiviert, behält Speichern unter unterstützte EXIF-Tags, einschließlich GPS.",
    },
    Message {
        english: "Off by default. Save As removes supported EXIF metadata, including GPS and camera identifiers. This choice lasts only for this session.",
        spanish: "Desactivado de forma predeterminada. Guardar como quita los metadatos EXIF compatibles, incluidos GPS e identificadores de cámara. Esta elección dura solo esta sesión.",
        french: "Désactivé par défaut. Enregistrer sous retire les métadonnées EXIF prises en charge, y compris le GPS et les identifiants d’appareil. Ce choix ne dure que pour cette session.",
        german: "Standardmäßig aus. Speichern unter entfernt unterstützte EXIF-Metadaten, einschließlich GPS und Kamerakennungen. Diese Wahl gilt nur für diese Sitzung.",
    },
    Message {
        english: "Play",
        spanish: "Reproducir",
        french: "Lecture",
        german: "Wiedergeben",
    },
    Message {
        english: "Pause",
        spanish: "Pausa",
        french: "Pause",
        german: "Pause",
    },
    Message {
        english: "Play animation",
        spanish: "Reproducir animación",
        french: "Lire l’animation",
        german: "Animation wiedergeben",
    },
    Message {
        english: "Pause animation",
        spanish: "Pausar animación",
        french: "Mettre l’animation en pause",
        german: "Animation anhalten",
    },
    Message {
        english: "Previous",
        spanish: "Anterior",
        french: "Précédent",
        german: "Zurück",
    },
    Message {
        english: "Next",
        spanish: "Siguiente",
        french: "Suivant",
        german: "Weiter",
    },
    Message {
        english: "Previous frame",
        spanish: "Fotograma anterior",
        french: "Image précédente",
        german: "Vorheriges Bild",
    },
    Message {
        english: "Next frame",
        spanish: "Fotograma siguiente",
        french: "Image suivante",
        german: "Nächstes Bild",
    },
    Message {
        english: "Frame {current} of {total}",
        spanish: "Fotograma {current} de {total}",
        french: "Image {current} sur {total}",
        german: "Bild {current} von {total}",
    },
    Message {
        english: "Page",
        spanish: "Página",
        french: "Page",
        german: "Seite",
    },
    Message {
        english: "Icon",
        spanish: "Icono",
        french: "Icône",
        german: "Symbol",
    },
    Message {
        english: "Previous {noun}",
        spanish: "{noun} anterior",
        french: "{noun} précédent",
        german: "Vorherige {noun}",
    },
    Message {
        english: "Next {noun}",
        spanish: "{noun} siguiente",
        french: "{noun} suivant",
        german: "Nächste {noun}",
    },
    Message {
        english: "Previous Page",
        spanish: "Página anterior",
        french: "Page précédente",
        german: "Vorherige Seite",
    },
    Message {
        english: "Previous page",
        spanish: "Página anterior",
        french: "Page précédente",
        german: "Vorherige Seite",
    },
    Message {
        english: "Next Page",
        spanish: "Página siguiente",
        french: "Page suivante",
        german: "Nächste Seite",
    },
    Message {
        english: "Next page",
        spanish: "Página siguiente",
        french: "Page suivante",
        german: "Nächste Seite",
    },
    Message {
        english: "Previous Icon",
        spanish: "Icono anterior",
        french: "Icône précédente",
        german: "Vorheriges Symbol",
    },
    Message {
        english: "Next Icon",
        spanish: "Icono siguiente",
        french: "Icône suivante",
        german: "Nächstes Symbol",
    },
    Message {
        english: "Previous Frame",
        spanish: "Fotograma anterior",
        french: "Image précédente",
        german: "Vorheriges Bild",
    },
    Message {
        english: "Next Frame",
        spanish: "Fotograma siguiente",
        french: "Image suivante",
        german: "Nächstes Bild",
    },
    Message {
        english: "Play Animation",
        spanish: "Reproducir animación",
        french: "Lire l’animation",
        german: "Animation wiedergeben",
    },
    Message {
        english: "Pause Animation",
        spanish: "Pausar animación",
        french: "Mettre l’animation en pause",
        german: "Animation anhalten",
    },
    Message {
        english: "Exit Fullscreen",
        spanish: "Salir de pantalla completa",
        french: "Quitter le plein écran",
        german: "Vollbild beenden",
    },
    Message {
        english: "Used for future folders, launches, Folder Previews, and full-image collage groups. The file you open stays selected.",
        spanish: "Se usa para carpetas futuras, inicios, vistas previas de carpeta y grupos de collage de imágenes completas. El archivo que abre permanece seleccionado.",
        french: "Utilisé pour les dossiers futurs, les lancements, les aperçus de dossier et les groupes de mosaïque d’images complètes. Le fichier que vous ouvrez reste sélectionné.",
        german: "Gilt für künftige Ordner, Starts, Ordnervorschauen und Collage-Gruppen. Die geöffnete Datei bleibt ausgewählt.",
    },
    Message {
        english: "Latest First uses file modification time. Name uses natural filename order.",
        spanish: "Más recientes primero usa la fecha de modificación. Nombre usa el orden natural del nombre de archivo.",
        french: "Plus récents d’abord utilise la date de modification. Nom utilise l’ordre naturel du nom de fichier.",
        german: "Neueste zuerst verwendet die Änderungszeit. Name verwendet die natürliche Dateinamenreihenfolge.",
    },
    Message {
        english: "Replace existing file?",
        spanish: "¿Reemplazar el archivo existente?",
        french: "Remplacer le fichier existant ?",
        german: "Vorhandene Datei ersetzen?",
    },
    Message {
        english: "The selected Save As destination exists. Replace that exact file with this exported copy?",
        spanish: "El destino de Guardar como ya existe. ¿Reemplazar exactamente ese archivo con esta copia exportada?",
        french: "La destination d’Enregistrer sous existe. Remplacer exactement ce fichier par cette copie exportée ?",
        german: "Das Ziel von Speichern unter existiert bereits. Diese Datei durch diese exportierte Kopie ersetzen?",
    },
    Message {
        english: "viewr rechecks this exact file immediately before replacement and stops if that check detects a change.",
        spanish: "viewr vuelve a comprobar exactamente este archivo justo antes de reemplazarlo y se detiene si esa comprobación detecta un cambio.",
        french: "viewr revérifie exactement ce fichier juste avant le remplacement et s’arrête si cette vérification détecte un changement.",
        german: "viewr prüft genau diese Datei unmittelbar vor dem Ersetzen erneut und bricht ab, wenn diese Prüfung eine Änderung erkennt.",
    },
    Message {
        english: "Replace file",
        spanish: "Reemplazar archivo",
        french: "Remplacer le fichier",
        german: "Datei ersetzen",
    },
    Message {
        english: "Update viewr",
        spanish: "Actualizar viewr",
        french: "Mettre à jour viewr",
        german: "viewr aktualisieren",
    },
    Message {
        english: "Current version: {version}",
        spanish: "Versión actual: {version}",
        french: "Version actuelle : {version}",
        german: "Aktuelle Version: {version}",
    },
    Message {
        english: "viewr never checks for or downloads updates by itself.",
        spanish: "viewr nunca busca ni descarga actualizaciones por sí mismo.",
        french: "viewr ne recherche ni ne télécharge jamais de mises à jour de lui-même.",
        german: "viewr prüft nicht selbst auf Updates und lädt keine herunter.",
    },
    Message {
        english: "Updates are explicit and come from the official GitHub release.",
        spanish: "Las actualizaciones son explícitas y proceden de la versión oficial de GitHub.",
        french: "Les mises à jour sont explicites et proviennent de la version officielle GitHub.",
        german: "Updates sind ausdrücklich und stammen aus der offiziellen GitHub-Version.",
    },
    Message {
        english: "Open the latest stable release in your browser, review its version and checksums, then close viewr before installing it.",
        spanish: "Abra la última versión estable en el navegador, revise su versión y sumas de comprobación, y cierre viewr antes de instalarla.",
        french: "Ouvrez la dernière version stable dans votre navigateur, vérifiez sa version et ses sommes de contrôle, puis fermez viewr avant de l’installer.",
        german: "Öffnen Sie die neueste stabile Version im Browser, prüfen Sie Version und Prüfsummen und schließen Sie viewr vor der Installation.",
    },
    Message {
        english: "This hands off only the release URL to your default browser. viewr itself does not connect to GitHub or run an updater.",
        spanish: "Esto entrega solo la URL de la versión a su navegador predeterminado. viewr no se conecta a GitHub ni ejecuta un actualizador.",
        french: "Cela transmet uniquement l’URL de la version à votre navigateur par défaut. viewr lui-même ne se connecte pas à GitHub et n’exécute pas de programme de mise à jour.",
        german: "Dies übergibt nur die Versions-URL an Ihren Standardbrowser. viewr selbst verbindet sich nicht mit GitHub und führt keinen Updater aus.",
    },
    Message {
        english: "Preparing a display-sized preview in the background",
        spanish: "Preparando en segundo plano una vista previa del tamaño de la pantalla",
        french: "Préparation en arrière-plan d’un aperçu à la taille de l’écran",
        german: "Vorschau in Bildschirmgröße wird im Hintergrund vorbereitet",
    },
    Message {
        english: "Crop applied",
        spanish: "Recorte aplicado",
        french: "Recadrage appliqué",
        german: "Zuschnitt angewendet",
    },
    Message {
        english: "Full image shown as a GPU-limited preview; export remains full resolution",
        spanish: "La imagen completa se muestra como vista previa limitada por la GPU; la exportación conserva la resolución completa",
        french: "Image entière affichée en aperçu limité par le GPU ; l’export conserve la pleine résolution",
        german: "Vollständiges Bild als GPU-begrenzte Vorschau angezeigt; der Export behält die volle Auflösung",
    },
    Message {
        english: "Open an image before assigning a rating",
        spanish: "Abra una imagen antes de asignar una valoración",
        french: "Ouvrez une image avant d’attribuer une note",
        german: "Öffnen Sie ein Bild, bevor Sie eine Bewertung vergeben",
    },
    Message {
        english: "Wait for the selected image to finish loading",
        spanish: "Espere a que la imagen seleccionada termine de cargarse",
        french: "Attendez la fin du chargement de l’image sélectionnée",
        german: "Warten Sie, bis das ausgewählte Bild vollständig geladen ist",
    },
    Message {
        english: "viewr could not verify this image's source safely. The file was not changed.",
        spanish: "viewr no pudo verificar de forma segura el origen de esta imagen. El archivo no se modificó.",
        french: "viewr n’a pas pu vérifier la source de cette image en toute sécurité. Le fichier n’a pas été modifié.",
        german: "viewr konnte die Quelle dieses Bildes nicht sicher prüfen. Die Datei wurde nicht geändert.",
    },
    Message {
        english: "The rating could not be read. Close and reopen viewr before changing this file.",
        spanish: "No se pudo leer la valoración. Cierre y vuelva a abrir viewr antes de cambiar este archivo.",
        french: "La note n’a pas pu être lue. Fermez puis rouvrez viewr avant de modifier ce fichier.",
        german: "Die Bewertung konnte nicht gelesen werden. Schließen Sie viewr und öffnen Sie es erneut, bevor Sie diese Datei ändern.",
    },
    Message {
        english: "The selected image changed before the rating could be saved",
        spanish: "La imagen seleccionada cambió antes de que se pudiera guardar la valoración",
        french: "L’image sélectionnée a changé avant l’enregistrement de la note",
        german: "Das ausgewählte Bild hat sich geändert, bevor die Bewertung gespeichert werden konnte",
    },
    Message {
        english: "Rating cleared.",
        spanish: "Valoración eliminada.",
        french: "Note effacée.",
        german: "Bewertung entfernt.",
    },
    Message {
        english: "Rating {rating} of 5 saved.",
        spanish: "Valoración {rating} de 5 guardada.",
        french: "Note {rating} sur 5 enregistrée.",
        german: "Bewertung {rating} von 5 gespeichert.",
    },
    Message {
        english: "Could not finish reading folder ratings. Showing all images.",
        spanish: "No se pudieron terminar de leer las valoraciones de la carpeta. Se muestran todas las imágenes.",
        french: "Impossible de terminer la lecture des notes du dossier. Toutes les images sont affichées.",
        german: "Die Ordnerbewertungen konnten nicht vollständig gelesen werden. Alle Bilder werden angezeigt.",
    },
    Message {
        english: "Reloading file from disk",
        spanish: "Volviendo a cargar el archivo desde el disco",
        french: "Rechargement du fichier depuis le disque",
        german: "Datei wird vom Datenträger neu geladen",
    },
    Message {
        english: "Open With requires the current image to finish loading",
        spanish: "Abrir con requiere que la imagen actual termine de cargarse",
        french: "Ouvrir avec nécessite que l’image actuelle ait fini de se charger",
        german: "Öffnen mit erfordert, dass das aktuelle Bild vollständig geladen ist",
    },
    Message {
        english: "Could not verify the current source for Open With",
        spanish: "No se pudo verificar el origen actual para Abrir con",
        french: "Impossible de vérifier la source actuelle pour Ouvrir avec",
        german: "Die aktuelle Quelle konnte für Öffnen mit nicht geprüft werden",
    },
    Message {
        english: "Could not start source verification for Open With",
        spanish: "No se pudo iniciar la verificación del origen para Abrir con",
        french: "Impossible de lancer la vérification de la source pour Ouvrir avec",
        german: "Die Quellprüfung für Öffnen mit konnte nicht gestartet werden",
    },
    Message {
        english: "Verifying source for Open With",
        spanish: "Verificando el origen para Abrir con",
        french: "Vérification de la source pour Ouvrir avec",
        german: "Quelle wird für Öffnen mit geprüft",
    },
    Message {
        english: "Could not finish source verification for Open With",
        spanish: "No se pudo completar la verificación del origen para Abrir con",
        french: "Impossible de terminer la vérification de la source pour Ouvrir avec",
        german: "Die Quellprüfung für Öffnen mit konnte nicht abgeschlossen werden",
    },
    Message {
        english: "Source changed on disk. Press F5 before Open With",
        spanish: "El origen cambió en el disco. Pulse F5 antes de usar Abrir con",
        french: "La source a changé sur le disque. Appuyez sur F5 avant d’utiliser Ouvrir avec",
        german: "Die Quelle wurde auf dem Datenträger geändert. Drücken Sie F5, bevor Sie Öffnen mit verwenden",
    },
    Message {
        english: "Open With is unavailable for this linked or unsupported source",
        spanish: "Abrir con no está disponible para este origen vinculado o no compatible",
        french: "Ouvrir avec n’est pas disponible pour cette source liée ou non prise en charge",
        german: "Öffnen mit ist für diese verknüpfte oder nicht unterstützte Quelle nicht verfügbar",
    },
    Message {
        english: "Source opened in another app. Changes reload when that is safe",
        spanish: "Origen abierto en otra aplicación. Los cambios se vuelven a cargar cuando sea seguro",
        french: "Source ouverte dans une autre application. Les modifications se rechargent lorsque c’est sans risque",
        german: "Quelle in einer anderen App geöffnet. Änderungen werden neu geladen, sobald das sicher ist",
    },
    Message {
        english: "Open With canceled",
        spanish: "Abrir con cancelado",
        french: "Ouvrir avec annulé",
        german: "Öffnen mit abgebrochen",
    },
    Message {
        english: "Could not open the app chooser",
        spanish: "No se pudo abrir el selector de aplicaciones",
        french: "Impossible d’ouvrir le sélecteur d’applications",
        german: "Die App-Auswahl konnte nicht geöffnet werden",
    },
    Message {
        english: "Full-image collage needs an open folder",
        spanish: "El collage de imágenes completas necesita una carpeta abierta",
        french: "La mosaïque d’images complètes nécessite un dossier ouvert",
        german: "Die Collage vollständiger Bilder benötigt einen geöffneten Ordner",
    },
    Message {
        english: "Full-image collage needs more than one matching photo",
        spanish: "El collage de imágenes completas necesita más de una foto coincidente",
        french: "La mosaïque d’images complètes nécessite plus d’une photo correspondante",
        german: "Die Collage vollständiger Bilder benötigt mehr als ein passendes Foto",
    },
    Message {
        english: "Full-image collage needs a selected photo",
        spanish: "El collage de imágenes completas necesita una foto seleccionada",
        french: "La mosaïque d’images complètes nécessite une photo sélectionnée",
        german: "Die Collage vollständiger Bilder benötigt ein ausgewähltes Foto",
    },
    Message {
        english: "Folder previews need more than one image",
        spanish: "Las vistas previas de carpeta necesitan más de una imagen",
        french: "Les aperçus du dossier nécessitent plus d’une image",
        german: "Ordnervorschauen benötigen mehr als ein Bild",
    },
    Message {
        english: "The Trash queue is full. Wait for the current moves to finish before continuing.",
        spanish: "La cola de la papelera está llena. Espere a que terminen los traslados actuales antes de continuar.",
        french: "La file de la corbeille est pleine. Attendez la fin des déplacements en cours avant de continuer.",
        german: "Die Papierkorb-Warteschlange ist voll. Warten Sie, bis die laufenden Verschiebungen abgeschlossen sind, bevor Sie fortfahren.",
    },
    Message {
        english: "Finishing spot heal in memory",
        spanish: "Terminando la corrección puntual en memoria",
        french: "Finalisation de la correction ponctuelle en mémoire",
        german: "Bereichsreparatur im Speicher wird abgeschlossen",
    },
    Message {
        english: "Spot Heal is unavailable for images larger than the GPU texture limit",
        spanish: "La corrección puntual no está disponible para imágenes que superan el límite de texturas de la GPU",
        french: "La correction ponctuelle n’est pas disponible pour les images dépassant la limite de texture du GPU",
        german: "Die Bereichsreparatur ist für Bilder über dem GPU-Texturlimit nicht verfügbar",
    },
    Message {
        english: "Apply a spot heal before refreshing its source",
        spanish: "Aplique una corrección puntual antes de actualizar su origen",
        french: "Appliquez une correction ponctuelle avant d’en actualiser la source",
        german: "Wenden Sie eine Bereichsreparatur an, bevor Sie deren Quelle aktualisieren",
    },
    Message {
        english: "No alternate spot-heal source is available",
        spanish: "No hay otro origen disponible para la corrección puntual",
        french: "Aucune autre source de correction ponctuelle n’est disponible",
        german: "Es ist keine alternative Quelle für die Bereichsreparatur verfügbar",
    },
    Message {
        english: "Spot-heal stroke is too long; use shorter strokes",
        spanish: "El trazo de corrección puntual es demasiado largo; use trazos más cortos",
        french: "Le tracé de correction ponctuelle est trop long ; utilisez des tracés plus courts",
        german: "Der Strich für die Bereichsreparatur ist zu lang; verwenden Sie kürzere Striche",
    },
    Message {
        english: "Spot heal stopped unexpectedly",
        spanish: "La corrección puntual se detuvo inesperadamente",
        french: "La correction ponctuelle s’est arrêtée de façon inattendue",
        german: "Die Bereichsreparatur wurde unerwartet beendet",
    },
    Message {
        english: "Undid spot heal",
        spanish: "Se deshizo la corrección puntual",
        french: "Correction ponctuelle annulée",
        german: "Bereichsreparatur rückgängig gemacht",
    },
    Message {
        english: "Redid spot heal",
        spanish: "Se rehízo la corrección puntual",
        french: "Correction ponctuelle rétablie",
        german: "Bereichsreparatur wiederhergestellt",
    },
    Message {
        english: "Permanently deleting file in the background",
        spanish: "Eliminando el archivo definitivamente en segundo plano",
        french: "Suppression définitive du fichier en arrière-plan",
        german: "Datei wird im Hintergrund endgültig gelöscht",
    },
    Message {
        english: "The selected image is no longer available, and no remaining image matches the rating filter.",
        spanish: "La imagen seleccionada ya no está disponible y ninguna imagen restante coincide con el filtro de valoración.",
        french: "L’image sélectionnée n’est plus disponible et aucune image restante ne correspond au filtre de note.",
        german: "Das ausgewählte Bild ist nicht mehr verfügbar, und kein verbleibendes Bild entspricht dem Bewertungsfilter.",
    },
    Message {
        english: "Could not start Trash restore. Undo receipts are unchanged; retry with U.",
        spanish: "No se pudo iniciar la restauración desde la papelera. Los comprobantes para deshacer no cambiaron; reintente con U.",
        french: "Impossible de lancer la restauration depuis la corbeille. Les reçus d’annulation sont inchangés ; réessayez avec U.",
        german: "Die Wiederherstellung aus dem Papierkorb konnte nicht gestartet werden. Die Rückgängig-Belege bleiben unverändert; versuchen Sie es erneut mit U.",
    },
    Message {
        english: "Pending Save As overwrite canceled because the active image selection changed.",
        spanish: "Se canceló la sobrescritura pendiente de Guardar como porque cambió la imagen seleccionada.",
        french: "Le remplacement en attente via Enregistrer sous a été annulé, car l’image sélectionnée a changé.",
        german: "Das ausstehende Überschreiben durch Speichern unter wurde abgebrochen, weil sich die Bildauswahl geändert hat.",
    },
    Message {
        english: "Pending rating change canceled because the active image was reopened or changed.",
        spanish: "Se canceló el cambio de valoración pendiente porque la imagen activa se volvió a abrir o cambió.",
        french: "Modification de note en attente annulée, car l’image active a été rouverte ou a changé.",
        german: "Ausstehende Bewertungsänderung abgebrochen, weil das aktive Bild erneut geöffnet oder geändert wurde.",
    },
    Message {
        english: "Save canceled. No file was changed.",
        spanish: "Guardado cancelado. No se modificó ningún archivo.",
        french: "Enregistrement annulé. Aucun fichier n’a été modifié.",
        german: "Speichern abgebrochen. Es wurde keine Datei geändert.",
    },
    Message {
        english: "Saving copy in the background",
        spanish: "Guardando la copia en segundo plano",
        french: "Enregistrement de la copie en arrière-plan",
        german: "Kopie wird im Hintergrund gespeichert",
    },
    Message {
        english: "The selected ratio is too large for this image",
        spanish: "La proporción seleccionada es demasiado grande para esta imagen",
        french: "Le format sélectionné est trop grand pour cette image",
        german: "Das gewählte Seitenverhältnis ist für dieses Bild zu groß",
    },
    Message {
        english: "Could not start crop. Selection kept; press Enter to try again.",
        spanish: "No se pudo iniciar el recorte. La selección se conserva; pulse Intro para intentarlo de nuevo.",
        french: "Impossible de lancer le recadrage. Sélection conservée ; appuyez sur Entrée pour réessayer.",
        german: "Der Zuschnitt konnte nicht gestartet werden. Die Auswahl bleibt erhalten; drücken Sie die Eingabetaste, um es erneut zu versuchen.",
    },
    Message {
        english: "Applying crop in the background",
        spanish: "Aplicando el recorte en segundo plano",
        french: "Application du recadrage en arrière-plan",
        german: "Zuschnitt wird im Hintergrund angewendet",
    },
    Message {
        english: "Finishing the rating update before closing...",
        spanish: "Terminando la actualización de la valoración antes de cerrar...",
        french: "Finalisation de la mise à jour de la note avant la fermeture...",
        german: "Bewertungsänderung wird vor dem Schließen abgeschlossen...",
    },
    Message {
        english: "Finishing Save As before closing...",
        spanish: "Terminando Guardar como antes de cerrar...",
        french: "Finalisation de l’opération Enregistrer sous avant la fermeture...",
        german: "Speichern unter wird vor dem Schließen abgeschlossen...",
    },
    Message {
        english: "Finishing Save As and the file operation before closing...",
        spanish: "Terminando Guardar como y la operación de archivo antes de cerrar...",
        french: "Finalisation de l’opération Enregistrer sous et de l’opération sur le fichier avant la fermeture...",
        german: "Speichern unter und der Dateivorgang werden vor dem Schließen abgeschlossen...",
    },
    Message {
        english: "Could not scan folder: {error}",
        spanish: "No se pudo examinar la carpeta: {error}",
        french: "Impossible d’analyser le dossier : {error}",
        german: "Der Ordner konnte nicht durchsucht werden: {error}",
    },
    Message {
        english: "{label}: {value}",
        spanish: "{label}: {value}",
        french: "{label} : {value}",
        german: "{label}: {value}",
    },
    Message {
        english: "Image details unavailable: {error}",
        spanish: "Detalles de la imagen no disponibles: {error}",
        french: "Détails de l’image indisponibles : {error}",
        german: "Bilddetails nicht verfügbar: {error}",
    },
    Message {
        english: "Animation unavailable; showing first frame: {error}",
        spanish: "Animación no disponible; se muestra el primer fotograma: {error}",
        french: "Animation indisponible ; affichage de la première image : {error}",
        german: "Animation nicht verfügbar; das erste Einzelbild wird angezeigt: {error}",
    },
    Message {
        english: "Pages unavailable; showing the first image: {error}",
        spanish: "Páginas no disponibles; se muestra la primera imagen: {error}",
        french: "Pages indisponibles ; affichage de la première image : {error}",
        german: "Seiten nicht verfügbar; das erste Bild wird angezeigt: {error}",
    },
    Message {
        english: "Container pages unavailable; showing the first image: {error}",
        spanish: "Páginas del contenedor no disponibles; se muestra la primera imagen: {error}",
        french: "Pages du conteneur indisponibles ; affichage de la première image : {error}",
        german: "Containerseiten nicht verfügbar; das erste Bild wird angezeigt: {error}",
    },
    Message {
        english: "Animation stopped: {error}",
        spanish: "Animación detenida: {error}",
        french: "Animation arrêtée : {error}",
        german: "Animation angehalten: {error}",
    },
    Message {
        english: "Could not show that page: {error}",
        spanish: "No se pudo mostrar esa página: {error}",
        french: "Impossible d’afficher cette page : {error}",
        german: "Diese Seite konnte nicht angezeigt werden: {error}",
    },
    Message {
        english: "Could not update display color: {error}",
        spanish: "No se pudo actualizar el color de la pantalla: {error}",
        french: "Impossible de mettre à jour la couleur de l’écran : {error}",
        german: "Die Bildschirmfarbe konnte nicht aktualisiert werden: {error}",
    },
    Message {
        english: "Could not continue moving files to Trash. 1 queued file was not moved.",
        spanish: "No se pudieron seguir moviendo archivos a la papelera. Un archivo en cola no se movió.",
        french: "Impossible de continuer la mise à la corbeille. Un fichier en attente n’a pas été déplacé.",
        german: "Das Verschieben in den Papierkorb konnte nicht fortgesetzt werden. Eine Datei in der Warteschlange wurde nicht verschoben.",
    },
    Message {
        english: "Could not continue moving files to Trash. {count} queued files were not moved.",
        spanish: "No se pudieron seguir moviendo archivos a la papelera. {count} archivos en cola no se movieron.",
        french: "Impossible de continuer la mise à la corbeille. {count} fichiers en attente n’ont pas été déplacés.",
        german: "Das Verschieben in den Papierkorb konnte nicht fortgesetzt werden. {count} Dateien in der Warteschlange wurden nicht verschoben.",
    },
    Message {
        english: "{failure} 1 queued file was not sent to Trash.",
        spanish: "{failure} Un archivo en cola no se envió a la papelera.",
        french: "{failure} Un fichier en attente n’a pas été mis à la corbeille.",
        german: "{failure} Eine Datei in der Warteschlange wurde nicht in den Papierkorb verschoben.",
    },
    Message {
        english: "{failure} {count} queued files were not sent to Trash.",
        spanish: "{failure} {count} archivos en cola no se enviaron a la papelera.",
        french: "{failure} {count} fichiers en attente n’ont pas été mis à la corbeille.",
        german: "{failure} {count} Dateien in der Warteschlange wurden nicht in den Papierkorb verschoben.",
    },
    Message {
        english: "The move to Trash needs attention.",
        spanish: "El traslado a la papelera requiere atención.",
        french: "La mise à la corbeille nécessite votre attention.",
        german: "Das Verschieben in den Papierkorb erfordert Ihre Aufmerksamkeit.",
    },
    Message {
        english: "Could not refresh heal source: {error}",
        spanish: "No se pudo actualizar el origen de la corrección: {error}",
        french: "Impossible d’actualiser la source de correction : {error}",
        german: "Die Reparaturquelle konnte nicht aktualisiert werden: {error}",
    },
    Message {
        english: "Could not start spot heal: {error}",
        spanish: "No se pudo iniciar la corrección puntual: {error}",
        french: "Impossible de lancer la correction ponctuelle : {error}",
        german: "Die Bereichsreparatur konnte nicht gestartet werden: {error}",
    },
    Message {
        english: "Spot heal failed: {error}",
        spanish: "La corrección puntual falló: {error}",
        french: "Échec de la correction ponctuelle : {error}",
        german: "Bereichsreparatur fehlgeschlagen: {error}",
    },
    Message {
        english: "Save failed: {error}",
        spanish: "Error al guardar: {error}",
        french: "Échec de l’enregistrement : {error}",
        german: "Speichern fehlgeschlagen: {error}",
    },
    Message {
        english: "Could not start save: {error}",
        spanish: "No se pudo iniciar el guardado: {error}",
        french: "Impossible de lancer l’enregistrement : {error}",
        german: "Das Speichern konnte nicht gestartet werden: {error}",
    },
    Message {
        english: "Could not decode: {error}. The previous image remains visible; Retry is available.",
        spanish: "No se pudo decodificar: {error}. La imagen anterior sigue visible; puede usar Reintentar.",
        french: "Impossible de décoder : {error}. L’image précédente reste visible ; vous pouvez utiliser Réessayer.",
        german: "Dekodierung nicht möglich: {error}. Das vorherige Bild bleibt sichtbar; Sie können „Erneut versuchen“ verwenden.",
    },
    Message {
        english: "Could not decode: {error}. Retry is available.",
        spanish: "No se pudo decodificar: {error}. Puede usar Reintentar.",
        french: "Impossible de décoder : {error}. Vous pouvez utiliser Réessayer.",
        german: "Dekodierung nicht möglich: {error}. Sie können „Erneut versuchen“ verwenden.",
    },
    Message {
        english: "Heal source {index} of {count}",
        spanish: "Origen de corrección {index} de {count}",
        french: "Source de correction {index} sur {count}",
        german: "Reparaturquelle {index} von {count}",
    },
    Message {
        english: "Spot healed in memory. Use Save As to keep it; Undo is available.",
        spanish: "Corrección puntual aplicada en memoria. Use Guardar como para conservarla; puede deshacer.",
        french: "Correction ponctuelle appliquée en mémoire. Utilisez Enregistrer sous pour la conserver ; l’annulation est disponible.",
        german: "Bereichsreparatur im Speicher angewendet. Verwenden Sie Speichern unter, um sie zu behalten; Rückgängig ist verfügbar.",
    },
    Message {
        english: "Saved copy",
        spanish: "Copia guardada",
        french: "Copie enregistrée",
        german: "Kopie gespeichert",
    },
    Message {
        english: "Saved edited copy",
        spanish: "Copia editada guardada",
        french: "Copie modifiée enregistrée",
        german: "Bearbeitete Kopie gespeichert",
    },
    Message {
        english: "EXIF retained",
        spanish: "EXIF conservado",
        french: "Données EXIF conservées",
        german: "EXIF beibehalten",
    },
    Message {
        english: "no EXIF found",
        spanish: "sin EXIF",
        french: "aucune donnée EXIF",
        german: "keine EXIF-Daten gefunden",
    },
    Message {
        english: "metadata stripped",
        spanish: "metadatos eliminados",
        french: "métadonnées supprimées",
        german: "Metadaten entfernt",
    },
    Message {
        english: "Saved copies will keep camera metadata (session only)",
        spanish: "Las copias guardadas conservarán los metadatos de la cámara (solo en esta sesión)",
        french: "Les copies enregistrées conserveront les métadonnées de l’appareil (session uniquement)",
        german: "Gespeicherte Kopien behalten die Kamerametadaten (nur in dieser Sitzung)",
    },
    Message {
        english: "Saved copies will strip camera metadata (default)",
        spanish: "Las copias guardadas eliminarán los metadatos de la cámara (predeterminado)",
        french: "Les copies enregistrées supprimeront les métadonnées de l’appareil (par défaut)",
        german: "Gespeicherte Kopien entfernen die Kamerametadaten (Standard)",
    },
    Message {
        english: "Finish or discard the current edit before changing pages.",
        spanish: "Termine o descarte la edición actual antes de cambiar de página.",
        french: "Terminez ou abandonnez la modification en cours avant de changer de page.",
        german: "Schließen Sie die aktuelle Bearbeitung ab oder verwerfen Sie sie, bevor Sie die Seite wechseln.",
    },
    Message {
        english: "Appearance changed for this session but could not be remembered. Check local configuration storage, then choose it again.",
        spanish: "La apariencia cambió para esta sesión, pero no se pudo recordar. Revise el almacenamiento de configuración local y vuelva a elegirla.",
        french: "L’apparence a changé pour cette session, mais n’a pas pu être mémorisée. Vérifiez le stockage de configuration local, puis choisissez-la de nouveau.",
        german: "Das Erscheinungsbild wurde für diese Sitzung geändert, konnte aber nicht gespeichert werden. Prüfen Sie den lokalen Konfigurationsspeicher und wählen Sie es erneut.",
    },
    Message {
        english: "Folder sort changed for this session but could not be remembered. Check local configuration storage, then choose it again.",
        spanish: "El orden de carpeta cambió para esta sesión, pero no se pudo recordar. Revise el almacenamiento de configuración local y vuelva a elegirlo.",
        french: "Le tri du dossier a changé pour cette session, mais n’a pas pu être mémorisé. Vérifiez le stockage de configuration local, puis choisissez-le de nouveau.",
        german: "Die Ordnersortierung wurde für diese Sitzung geändert, konnte aber nicht gespeichert werden. Prüfen Sie den lokalen Konfigurationsspeicher und wählen Sie sie erneut.",
    },
    Message {
        english: "Language changed for this session but could not be remembered. Check local configuration storage, then choose it again.",
        spanish: "El idioma cambió para esta sesión, pero no se pudo recordar. Revise el almacenamiento de configuración local y vuelva a elegirlo.",
        french: "La langue a changé pour cette session, mais n’a pas pu être mémorisée. Vérifiez le stockage de configuration local, puis choisissez-la de nouveau.",
        german: "Die Sprache wurde für diese Sitzung geändert, konnte aber nicht gespeichert werden. Prüfen Sie den lokalen Konfigurationsspeicher und wählen Sie sie erneut.",
    },
    Message {
        english: "The selected image is no longer available",
        spanish: "La imagen seleccionada ya no está disponible",
        french: "L’image sélectionnée n’est plus disponible",
        german: "Das ausgewählte Bild ist nicht mehr verfügbar",
    },
    Message {
        english: "Could not start the move to Trash. Nothing was moved.",
        spanish: "No se pudo iniciar el traslado a la papelera. No se movió nada.",
        french: "Impossible de lancer la mise à la corbeille. Rien n’a été déplacé.",
        german: "Das Verschieben in den Papierkorb konnte nicht gestartet werden. Es wurde nichts verschoben.",
    },
    Message {
        english: "Could not start the next queued move to Trash. That file was not moved.",
        spanish: "No se pudo iniciar el siguiente traslado en cola a la papelera. Ese archivo no se movió.",
        french: "Impossible de lancer la prochaine mise à la corbeille en attente. Ce fichier n’a pas été déplacé.",
        german: "Das nächste Verschieben in den Papierkorb aus der Warteschlange konnte nicht gestartet werden. Diese Datei wurde nicht verschoben.",
    },
    Message {
        english: "Could not start permanent delete. Nothing was deleted.",
        spanish: "No se pudo iniciar la eliminación definitiva. No se eliminó nada.",
        french: "Impossible de lancer la suppression définitive. Rien n’a été supprimé.",
        german: "Das endgültige Löschen konnte nicht gestartet werden. Es wurde nichts gelöscht.",
    },
    Message {
        english: "Could not display image: {error}",
        spanish: "No se pudo mostrar la imagen: {error}",
        french: "Impossible d’afficher l’image : {error}",
        german: "Das Bild konnte nicht angezeigt werden: {error}",
    },
    Message {
        english: "Could not prepare image preview: {error}",
        spanish: "No se pudo preparar la vista previa de la imagen: {error}",
        french: "Impossible de préparer l’aperçu de l’image : {error}",
        german: "Die Bildvorschau konnte nicht vorbereitet werden: {error}",
    },
    Message {
        english: "Could not start image decode: {error}",
        spanish: "No se pudo iniciar la decodificación de la imagen: {error}",
        french: "Impossible de lancer le décodage de l’image : {error}",
        german: "Die Bilddekodierung konnte nicht gestartet werden: {error}",
    },
    Message {
        english: "Folder is too large for safe automatic browsing. Browsing only this file. Use Open Folder to browse the rest.",
        spanish: "La carpeta es demasiado grande para examinarla automáticamente de forma segura. Solo se muestra este archivo. Use Abrir carpeta para examinar el resto.",
        french: "Le dossier est trop volumineux pour une navigation automatique sûre. Navigation limitée à ce fichier. Utilisez Ouvrir un dossier pour parcourir le reste.",
        german: "Der Ordner ist für sicheres automatisches Durchsuchen zu groß. Nur diese Datei wird angezeigt. Verwenden Sie Ordner öffnen, um den Rest zu durchsuchen.",
    },
    Message {
        english: "Folder browsing is unavailable. Browsing only this file. Use Open Folder to browse the rest.",
        spanish: "No se puede examinar la carpeta. Solo se muestra este archivo. Use Abrir carpeta para examinar el resto.",
        french: "La navigation dans le dossier est indisponible. Navigation limitée à ce fichier. Utilisez Ouvrir un dossier pour parcourir le reste.",
        german: "Das Durchsuchen des Ordners ist nicht verfügbar. Nur diese Datei wird angezeigt. Verwenden Sie Ordner öffnen, um den Rest zu durchsuchen.",
    },
    Message {
        english: "The selected image is no longer available. Opening the first image in the folder.",
        spanish: "La imagen seleccionada ya no está disponible. Se abre la primera imagen de la carpeta.",
        french: "L’image sélectionnée n’est plus disponible. Ouverture de la première image du dossier.",
        german: "Das ausgewählte Bild ist nicht mehr verfügbar. Das erste Bild im Ordner wird geöffnet.",
    },
    Message {
        english: "The selected image is no longer available, and the folder contains no other supported images.",
        spanish: "La imagen seleccionada ya no está disponible y la carpeta no contiene otras imágenes compatibles.",
        french: "L’image sélectionnée n’est plus disponible et le dossier ne contient aucune autre image prise en charge.",
        german: "Das ausgewählte Bild ist nicht mehr verfügbar, und der Ordner enthält keine anderen unterstützten Bilder.",
    },
    Message {
        english: "The selected image is no longer available. The folder is too large to choose another image safely.",
        spanish: "La imagen seleccionada ya no está disponible. La carpeta es demasiado grande para elegir otra imagen de forma segura.",
        french: "L’image sélectionnée n’est plus disponible. Le dossier est trop volumineux pour choisir une autre image en toute sécurité.",
        german: "Das ausgewählte Bild ist nicht mehr verfügbar. Der Ordner ist zu groß, um sicher ein anderes Bild auszuwählen.",
    },
    Message {
        english: "The selected image is no longer available. Could not read the folder to choose another image.",
        spanish: "La imagen seleccionada ya no está disponible. No se pudo leer la carpeta para elegir otra imagen.",
        french: "L’image sélectionnée n’est plus disponible. Impossible de lire le dossier pour choisir une autre image.",
        german: "Das ausgewählte Bild ist nicht mehr verfügbar. Der Ordner konnte nicht gelesen werden, um ein anderes Bild auszuwählen.",
    },
    Message {
        english: "The selected folder contains no supported images",
        spanish: "La carpeta seleccionada no contiene imágenes compatibles",
        french: "Le dossier sélectionné ne contient aucune image prise en charge",
        german: "Der ausgewählte Ordner enthält keine unterstützten Bilder",
    },
    Message {
        english: "The selected folder exceeds safe browsing limits",
        spanish: "La carpeta seleccionada supera los límites de exploración segura",
        french: "Le dossier sélectionné dépasse les limites de navigation sûre",
        german: "Der ausgewählte Ordner überschreitet die Grenzen für sicheres Durchsuchen",
    },
    Message {
        english: "Could not read the selected folder. Try Open Folder again.",
        spanish: "No se pudo leer la carpeta seleccionada. Vuelva a intentarlo con Abrir carpeta.",
        french: "Impossible de lire le dossier sélectionné. Réessayez avec Ouvrir un dossier.",
        german: "Der ausgewählte Ordner konnte nicht gelesen werden. Versuchen Sie es erneut mit Ordner öffnen.",
    },
    Message {
        english: "Could not restore saved appearance. Using System.",
        spanish: "No se pudo restaurar la apariencia guardada. Se usa Sistema.",
        french: "Impossible de restaurer l’apparence enregistrée. Utilisation du réglage Système.",
        german: "Das gespeicherte Erscheinungsbild konnte nicht wiederhergestellt werden. System wird verwendet.",
    },
    Message {
        english: "Could not restore the saved language. Using System.",
        spanish: "No se pudo restaurar el idioma guardado. Se usa Sistema.",
        french: "Impossible de restaurer la langue enregistrée. Utilisation du réglage Système.",
        german: "Die gespeicherte Sprache konnte nicht wiederhergestellt werden. System wird verwendet.",
    },
    Message {
        english: "Could not restore saved folder sort. Using Latest First.",
        spanish: "No se pudo restaurar el orden de carpeta guardado. Se usa Más recientes primero.",
        french: "Impossible de restaurer le tri du dossier enregistré. Utilisation du tri Plus récents d’abord.",
        german: "Die gespeicherte Ordnersortierung konnte nicht wiederhergestellt werden. Neueste zuerst wird verwendet.",
    },
    Message {
        english: "Could not restore some saved preferences. Using safe system defaults for them.",
        spanish: "No se pudieron restaurar algunas preferencias guardadas. Se usan valores predeterminados seguros del sistema.",
        french: "Impossible de restaurer certaines préférences enregistrées. Utilisation de valeurs système sûres par défaut.",
        german: "Einige gespeicherte Einstellungen konnten nicht wiederhergestellt werden. Sichere Systemstandards werden verwendet.",
    },
    Message {
        english: "The image decoder stopped unexpectedly",
        spanish: "El decodificador de imágenes se detuvo inesperadamente",
        french: "Le décodeur d’images s’est arrêté de façon inattendue",
        german: "Der Bilddecoder wurde unerwartet beendet",
    },
    Message {
        english: "The image could not be decoded",
        spanish: "No se pudo decodificar la imagen",
        french: "L’image n’a pas pu être décodée",
        german: "Das Bild konnte nicht dekodiert werden",
    },
    Message {
        english: "Open one image. When access allows, viewr also browses supported images in its folder for this session.",
        spanish: "Abre una imagen. Si el acceso lo permite, viewr también examina las imágenes compatibles de su carpeta durante esta sesión.",
        french: "Ouvre une image. Si l’accès le permet, viewr parcourt aussi les images prises en charge de son dossier pendant cette session.",
        german: "Öffnet ein Bild. Sofern der Zugriff es erlaubt, durchsucht viewr in dieser Sitzung auch die unterstützten Bilder in seinem Ordner.",
    },
    Message {
        english: "Choose a folder explicitly and browse its supported images for this session.",
        spanish: "Elija una carpeta de forma explícita y examine sus imágenes compatibles durante esta sesión.",
        french: "Choisissez explicitement un dossier et parcourez ses images prises en charge pendant cette session.",
        german: "Wählen Sie ausdrücklich einen Ordner und durchsuchen Sie seine unterstützten Bilder in dieser Sitzung.",
    },
    Message {
        english: "Opens the original file, including embedded metadata, in an app you choose. Unsaved viewr edits are not included. That app's privacy rules apply. If the other app changes the file, viewr reloads it when that is safe, or asks you to press F5 when unsaved edits would be lost.",
        spanish: "Abre el archivo original, con sus metadatos incrustados, en la aplicación que elija. No incluye las ediciones de viewr sin guardar. Se aplican las reglas de privacidad de esa aplicación. Si la otra aplicación cambia el archivo, viewr lo vuelve a cargar cuando es seguro, o le pide que pulse F5 si se perderían ediciones sin guardar.",
        french: "Ouvre le fichier d’origine, métadonnées intégrées comprises, dans l’application de votre choix. Les modifications non enregistrées de viewr ne sont pas incluses. Les règles de confidentialité de cette application s’appliquent. Si l’autre application modifie le fichier, viewr le recharge lorsque c’est sans risque, ou vous demande d’appuyer sur F5 si des modifications non enregistrées seraient perdues.",
        german: "Öffnet die Originaldatei samt eingebetteter Metadaten in einer App Ihrer Wahl. Nicht gespeicherte Bearbeitungen in viewr sind nicht enthalten. Es gelten die Datenschutzregeln dieser App. Ändert die andere App die Datei, lädt viewr sie neu, sobald das sicher ist, oder bittet Sie, F5 zu drücken, wenn sonst nicht gespeicherte Bearbeitungen verloren gingen.",
    },
    Message {
        english: "Changes app chrome and its default canvas. Image pixels stay unchanged; Image Background overrides the canvas separately.",
        spanish: "Cambia la interfaz de la aplicación y su lienzo predeterminado. Los píxeles de la imagen no cambian; Fondo de imagen reemplaza el lienzo por separado.",
        french: "Modifie l’interface de l’application et son canevas par défaut. Les pixels de l’image restent inchangés ; Arrière-plan de l’image remplace le canevas séparément.",
        german: "Ändert die App-Oberfläche und ihre Standardfläche. Die Bildpixel bleiben unverändert; Bildhintergrund ersetzt die Fläche separat.",
    },
    Message {
        english: "Photo {position} of {total} in the active folder view",
        spanish: "Foto {position} de {total} en la vista de carpeta activa",
        french: "Photo {position} sur {total} dans la vue du dossier active",
        german: "Foto {position} von {total} in der aktiven Ordneransicht",
    },
    Message {
        english: "{status}  |  Left/Right select  |  Down/Enter opens  |  Page Up/Down groups  |  Esc returns",
        spanish: "{status}  |  Left/Right selecciona  |  Down/Enter abre  |  Page Up/Down cambia de grupo  |  Esc vuelve",
        french: "{status}  |  Left/Right sélectionne  |  Down/Enter ouvre  |  Page Up/Down change de groupe  |  Esc revient",
        german: "{status}  |  Left/Right wählt  |  Down/Enter öffnet  |  Page Up/Down wechselt die Gruppe  |  Esc kehrt zurück",
    },
    Message {
        english: "Full-image collage  {ready} of {target} photos ready",
        spanish: "Collage de imágenes completas  {ready} de {target} fotos listas",
        french: "Mosaïque d’images complètes  {ready} photos prêtes sur {target}",
        german: "Collage vollständiger Bilder  {ready} von {target} Fotos bereit",
    },
    Message {
        english: "Full-image collage  {ready} of {target} photos fit the 256 MiB memory limit",
        spanish: "Collage de imágenes completas  {ready} de {target} fotos caben en el límite de memoria de 256 MiB",
        french: "Mosaïque d’images complètes  {ready} photos sur {target} tiennent dans la limite de mémoire de 256 Mio",
        german: "Collage vollständiger Bilder  {ready} von {target} Fotos passen in das Speicherlimit von 256 MiB",
    },
    Message {
        english: "Full-image collage  {ready} of {target} photos meet full-image display limits",
        spanish: "Collage de imágenes completas  {ready} de {target} fotos cumplen los límites de visualización completa",
        french: "Mosaïque d’images complètes  {ready} photos sur {target} respectent les limites d’affichage complet",
        german: "Collage vollständiger Bilder  {ready} von {target} Fotos erfüllen die Grenzen für vollständige Anzeige",
    },
    Message {
        english: "Full-image collage  {ready} of {target} photos available",
        spanish: "Collage de imágenes completas  {ready} de {target} fotos disponibles",
        french: "Mosaïque d’images complètes  {ready} photos disponibles sur {target}",
        german: "Collage vollständiger Bilder  {ready} von {target} Fotos verfügbar",
    },
    Message {
        english: "Full-image collage  {ready} photos",
        spanish: "Collage de imágenes completas  {ready} fotos",
        french: "Mosaïque d’images complètes  {ready} photos",
        german: "Collage vollständiger Bilder  {ready} Fotos",
    },
    Message {
        english: "Full-image collage loading complete photos",
        spanish: "Collage de imágenes completas: cargando fotos completas",
        french: "Mosaïque d’images complètes : chargement des photos complètes",
        german: "Collage vollständiger Bilder: vollständige Fotos werden geladen",
    },
    Message {
        english: "Quick Tools",
        spanish: "Herramientas rápidas",
        french: "Outils rapides",
        german: "Schnellwerkzeuge",
    },
    Message {
        english: "Heal Brush Radius",
        spanish: "Radio del pincel de corrección",
        french: "Rayon du pinceau de correction",
        german: "Pinselradius der Reparatur",
    },
    Message {
        english: "Heal brush radius",
        spanish: "Radio del pincel de corrección",
        french: "Rayon du pinceau de correction",
        german: "Pinselradius der Reparatur",
    },
    Message {
        english: "Heal Feather",
        spanish: "Difuminado de la corrección",
        french: "Contour progressif de la correction",
        german: "Randweichheit der Reparatur",
    },
    Message {
        english: "Heal feather",
        spanish: "Difuminado de la corrección",
        french: "Contour progressif de la correction",
        german: "Randweichheit der Reparatur",
    },
    Message {
        english: "Choose whether PNG, JPEG, or other image types open with viewr.",
        spanish: "Elija si PNG, JPEG u otros tipos de imagen se abren con viewr.",
        french: "Choisissez si les PNG, JPEG ou d’autres types d’image s’ouvrent avec viewr.",
        german: "Legen Sie fest, ob PNG, JPEG oder andere Bildtypen mit viewr geöffnet werden.",
    },
    Message {
        english: "Latest First uses file modification time. The selection becomes the default for future folders and launches. This viewr build does not receive the file manager's current sort when an image opens.",
        spanish: "Más recientes primero usa la fecha de modificación del archivo. La selección pasa a ser la predeterminada para futuras carpetas e inicios. Esta versión de viewr no recibe el orden actual del administrador de archivos al abrir una imagen.",
        french: "Plus récents d’abord utilise la date de modification du fichier. La sélection devient la valeur par défaut pour les prochains dossiers et lancements. Cette version de viewr ne reçoit pas le tri actuel du gestionnaire de fichiers à l’ouverture d’une image.",
        german: "Neueste zuerst verwendet die Änderungszeit der Datei. Die Auswahl wird zum Standard für künftige Ordner und Starts. Diese viewr-Version erhält beim Öffnen eines Bildes nicht die aktuelle Sortierung des Dateimanagers.",
    },
    Message {
        english: "Fit Image to View",
        spanish: "Ajustar imagen a la vista",
        french: "Ajuster l’image à la vue",
        german: "Bild an Ansicht anpassen",
    },
    Message {
        english: "Actual Size",
        spanish: "Tamaño real",
        french: "Taille réelle",
        german: "Originalgröße",
    },
    Message {
        english: "Zoom In",
        spanish: "Acercar",
        french: "Zoom avant",
        german: "Vergrößern",
    },
    Message {
        english: "Zoom Out",
        spanish: "Alejar",
        french: "Zoom arrière",
        german: "Verkleinern",
    },
    Message {
        english: "Toggle {panel} ({shortcut})",
        spanish: "Mostrar u ocultar {panel} ({shortcut})",
        french: "Afficher ou masquer {panel} ({shortcut})",
        german: "{panel} ein- oder ausblenden ({shortcut})",
    },
    Message {
        english: "Open the latest official GitHub release. No background check.",
        spanish: "Abre la última versión oficial en GitHub. Sin comprobación en segundo plano.",
        french: "Ouvre la dernière version officielle sur GitHub. Aucune vérification en arrière-plan.",
        german: "Öffnet die neueste offizielle Version auf GitHub. Keine Prüfung im Hintergrund.",
    },
    Message {
        english: "A private, local-first image viewer",
        spanish: "Un visor de imágenes privado y local",
        french: "Une visionneuse d’images privée et locale",
        german: "Ein privater, lokaler Bildbetrachter",
    },
    Message {
        english: "No network access",
        spanish: "Sin acceso a la red",
        french: "Aucun accès réseau",
        german: "Kein Netzwerkzugriff",
    },
    Message {
        english: "No telemetry, accounts, cloud sync, or background indexing.",
        spanish: "Sin telemetría, cuentas, sincronización en la nube ni indexación en segundo plano.",
        french: "Aucune télémétrie, aucun compte, aucune synchronisation cloud ni indexation en arrière-plan.",
        german: "Keine Telemetrie, keine Konten, keine Cloud-Synchronisierung und keine Indizierung im Hintergrund.",
    },
    Message {
        english: "Photos and edits stay local unless you explicitly save a copy.",
        spanish: "Las fotos y las ediciones permanecen en el equipo salvo que guarde una copia de forma explícita.",
        french: "Les photos et les modifications restent locales, sauf si vous enregistrez explicitement une copie.",
        german: "Fotos und Bearbeitungen bleiben lokal, außer Sie speichern ausdrücklich eine Kopie.",
    },
    Message {
        english: "Version",
        spanish: "Versión",
        french: "Version",
        german: "Version",
    },
    Message {
        english: "Platform",
        spanish: "Plataforma",
        french: "Plateforme",
        german: "Plattform",
    },
    Message {
        english: "License",
        spanish: "Licencia",
        french: "Licence",
        german: "Lizenz",
    },
    Message {
        english: "Shortcuts",
        spanish: "Atajos",
        french: "Raccourcis",
        german: "Tastenkürzel",
    },
    Message {
        english: "About viewr. Private local-first image viewer. No network access, telemetry, accounts, or background indexing.",
        spanish: "Acerca de viewr. Visor de imágenes privado y local. Sin acceso a la red, telemetría, cuentas ni indexación en segundo plano.",
        french: "À propos de viewr. Visionneuse d’images privée et locale. Aucun accès réseau, aucune télémétrie, aucun compte ni indexation en arrière-plan.",
        german: "Über viewr. Privater, lokaler Bildbetrachter. Kein Netzwerkzugriff, keine Telemetrie, keine Konten und keine Indizierung im Hintergrund.",
    },
    Message {
        english: "Update viewr. One explicit action opens the latest official GitHub release. No automatic network check or background updater.",
        spanish: "Actualizar viewr. Una acción explícita abre la última versión oficial en GitHub. Sin comprobación automática de red ni actualizador en segundo plano.",
        french: "Mettre à jour viewr. Une action explicite ouvre la dernière version officielle sur GitHub. Aucune vérification réseau automatique ni mise à jour en arrière-plan.",
        german: "viewr aktualisieren. Eine ausdrückliche Aktion öffnet die neueste offizielle Version auf GitHub. Keine automatische Netzwerkprüfung und kein Hintergrund-Updater.",
    },
    Message {
        english: "Preferences. Default folder sort and opt-in default image viewer settings.",
        spanish: "Preferencias. Orden de carpeta predeterminado y configuración opcional del visor de imágenes predeterminado.",
        french: "Préférences. Tri par défaut des dossiers et réglages facultatifs de la visionneuse d’images par défaut.",
        german: "Einstellungen. Standard-Ordnersortierung und optionale Einstellungen für die Standard-Bildanzeige.",
    },
    Message {
        english: "viewr never changes file associations during installation or startup.",
        spanish: "viewr nunca cambia las asociaciones de archivos durante la instalación ni al iniciarse.",
        french: "viewr ne modifie jamais les associations de fichiers pendant l’installation ou au démarrage.",
        german: "viewr ändert Dateizuordnungen niemals während der Installation oder beim Start.",
    },
    Message {
        english: "Defaults are selected per file type. Start with PNG and JPEG, then add only the formats you want viewr to open.",
        spanish: "Los valores predeterminados se eligen por tipo de archivo. Empiece con PNG y JPEG y añada solo los formatos que quiera abrir con viewr.",
        french: "Les valeurs par défaut se choisissent par type de fichier. Commencez par PNG et JPEG, puis ajoutez seulement les formats que viewr doit ouvrir.",
        german: "Standards werden pro Dateityp festgelegt. Beginnen Sie mit PNG und JPEG und fügen Sie dann nur die Formate hinzu, die viewr öffnen soll.",
    },
    Message {
        english: "This viewr build receives the selected file, but not the file manager's current folder sort. Use View > Folder Sort for Latest First or Name.",
        spanish: "Esta versión de viewr recibe el archivo seleccionado, pero no el orden de carpeta actual del administrador de archivos. Use Ver > Orden de carpeta para Más recientes primero o Nombre.",
        french: "Cette version de viewr reçoit le fichier sélectionné, mais pas le tri actuel du dossier dans le gestionnaire de fichiers. Utilisez Affichage > Tri du dossier pour Plus récents d’abord ou Nom.",
        german: "Diese viewr-Version erhält die ausgewählte Datei, aber nicht die aktuelle Ordnersortierung des Dateimanagers. Verwenden Sie Ansicht > Ordnersortierung für Neueste zuerst oder Name.",
    },
    Message {
        english: "Default image viewer. File associations change only after an explicit operating-system choice.",
        spanish: "Visor de imágenes predeterminado. Las asociaciones de archivos solo cambian tras una elección explícita en el sistema operativo.",
        french: "Visionneuse d’images par défaut. Les associations de fichiers ne changent qu’après un choix explicite dans le système d’exploitation.",
        german: "Standard-Bildanzeige. Dateizuordnungen ändern sich nur nach einer ausdrücklichen Auswahl im Betriebssystem.",
    },
    Message {
        english: "In Windows Default Apps, search for .png, .jpg, and .jpeg, then choose viewr for each type. If viewr is not listed, use a file's Open with menu, choose another app, browse to viewr.exe, and select Always.",
        spanish: "En Aplicaciones predeterminadas de Windows, busque .png, .jpg y .jpeg y elija viewr para cada tipo. Si viewr no aparece, use el menú Abrir con de un archivo, elija otra aplicación, busque viewr.exe y seleccione Siempre.",
        french: "Dans les paramètres Applications par défaut de Windows, recherchez .png, .jpg et .jpeg, puis choisissez viewr pour chaque type. Si viewr n’apparaît pas, utilisez le menu Ouvrir avec d’un fichier, choisissez une autre application, accédez à viewr.exe et sélectionnez Toujours.",
        german: "Suchen Sie in Windows unter Standard-Apps nach .png, .jpg und .jpeg und wählen Sie für jeden Typ viewr. Wird viewr nicht aufgeführt, öffnen Sie für eine Datei das Menü Öffnen mit, wählen Sie Andere App auswählen, navigieren Sie zu viewr.exe und wählen Sie Immer.",
    },
    Message {
        english: "Open Windows Default Apps",
        spanish: "Abrir Aplicaciones predeterminadas de Windows",
        french: "Ouvrir Applications par défaut de Windows",
        german: "Windows-Standard-Apps öffnen",
    },
    Message {
        english: "Use your desktop's file properties or Default Applications screen to choose viewr for PNG and JPEG. The viewr installer registers the desktop entry but does not change a default.",
        spanish: "Use las propiedades de archivo o la pantalla de aplicaciones predeterminadas de su escritorio para elegir viewr para PNG y JPEG. El instalador de viewr registra la entrada del escritorio, pero no cambia ningún valor predeterminado.",
        french: "Utilisez les propriétés de fichier ou l’écran des applications par défaut de votre bureau pour choisir viewr pour PNG et JPEG. Le programme d’installation de viewr enregistre l’entrée de bureau sans modifier de valeur par défaut.",
        german: "Verwenden Sie die Dateieigenschaften oder die Standardanwendungen Ihrer Arbeitsumgebung, um viewr für PNG und JPEG zu wählen. Das viewr-Installationsprogramm registriert den Desktop-Eintrag, ändert aber keinen Standard.",
    },
    Message {
        english: "Copy PNG/JPEG commands",
        spanish: "Copiar comandos PNG/JPEG",
        french: "Copier les commandes PNG/JPEG",
        german: "PNG/JPEG-Befehle kopieren",
    },
    Message {
        english: "For a viewr app bundle, select a PNG in Finder, choose File > Get Info, choose viewr under Open with, then select Change All. Repeat with a JPEG. Portable command-line builds are not app bundles and cannot appear as a Finder default.",
        spanish: "Para un paquete de aplicación de viewr, seleccione un PNG en el Finder, elija Archivo > Obtener información, elija viewr en Abrir con y seleccione Cambiar todo. Repita con un JPEG. Las versiones portátiles de línea de comandos no son paquetes de aplicación y no pueden aparecer como predeterminadas en el Finder.",
        french: "Pour un paquet d’application viewr, sélectionnez un PNG dans le Finder, choisissez Fichier > Lire les informations, choisissez viewr sous Ouvrir avec, puis sélectionnez Tout modifier. Répétez avec un JPEG. Les versions portables en ligne de commande ne sont pas des paquets d’application et ne peuvent pas apparaître comme application par défaut dans le Finder.",
        german: "Wählen Sie für ein viewr-App-Paket im Finder ein PNG, wählen Sie Ablage > Informationen, wählen Sie viewr unter Öffnen mit und dann Alle ändern. Wiederholen Sie das mit einem JPEG. Portable Befehlszeilenversionen sind keine App-Pakete und können im Finder nicht als Standard erscheinen.",
    },
    Message {
        english: "Use the operating system's default applications settings to choose viewr for PNG and JPEG.",
        spanish: "Use la configuración de aplicaciones predeterminadas del sistema operativo para elegir viewr para PNG y JPEG.",
        french: "Utilisez les réglages d’applications par défaut du système d’exploitation pour choisir viewr pour PNG et JPEG.",
        german: "Verwenden Sie die Standardanwendungs-Einstellungen des Betriebssystems, um viewr für PNG und JPEG zu wählen.",
    },
    Message {
        english: "Replace existing file? The selected Save As destination exists. Confirm replacement or cancel without changing it.",
        spanish: "¿Reemplazar el archivo existente? El destino elegido en Guardar como ya existe. Confirme el reemplazo o cancele sin modificarlo.",
        french: "Remplacer le fichier existant ? La destination choisie dans Enregistrer sous existe déjà. Confirmez le remplacement ou annulez sans le modifier.",
        german: "Vorhandene Datei ersetzen? Das in Speichern unter gewählte Ziel existiert bereits. Bestätigen Sie das Ersetzen oder brechen Sie ab, ohne es zu ändern.",
    },
    Message {
        english: "Clear this rating?",
        spanish: "¿Borrar esta valoración?",
        french: "Effacer cette note ?",
        german: "Diese Bewertung entfernen?",
    },
    Message {
        english: "Clear rating",
        spanish: "Borrar valoración",
        french: "Effacer la note",
        german: "Bewertung entfernen",
    },
    Message {
        english: "Save rating {rating} of 5?",
        spanish: "¿Guardar la valoración {rating} de 5?",
        french: "Enregistrer la note {rating} sur 5 ?",
        german: "Bewertung {rating} von 5 speichern?",
    },
    Message {
        english: "Save rating",
        spanish: "Guardar valoración",
        french: "Enregistrer la note",
        german: "Bewertung speichern",
    },
    Message {
        english: "Ratings are written into this image file and may be visible to other apps.",
        spanish: "Las valoraciones se escriben en este archivo de imagen y pueden ser visibles para otras aplicaciones.",
        french: "Les notes sont écrites dans ce fichier image et peuvent être visibles par d’autres applications.",
        german: "Bewertungen werden in diese Bilddatei geschrieben und können für andere Apps sichtbar sein.",
    },
    Message {
        english: "viewr updates embedded metadata in the source JPEG. It does not create a database or sidecar.",
        spanish: "viewr actualiza los metadatos incrustados en el JPEG de origen. No crea una base de datos ni un archivo auxiliar.",
        french: "viewr met à jour les métadonnées intégrées au JPEG d’origine. Il ne crée ni base de données ni fichier annexe.",
        german: "viewr aktualisiert die eingebetteten Metadaten im Quell-JPEG. Es erstellt keine Datenbank und keine Begleitdatei.",
    },
    Message {
        english: "{title}. Ratings are written into this image file and may be visible to other apps.",
        spanish: "{title}. Las valoraciones se escriben en este archivo de imagen y pueden ser visibles para otras aplicaciones.",
        french: "{title}. Les notes sont écrites dans ce fichier image et peuvent être visibles par d’autres applications.",
        german: "{title}. Bewertungen werden in diese Bilddatei geschrieben und können für andere Apps sichtbar sein.",
    },
    Message {
        english: "No images are rated {rating} or higher.",
        spanish: "Ninguna imagen tiene una valoración de {rating} o más.",
        french: "Aucune image n’a une note égale ou supérieure à {rating}.",
        german: "Kein Bild ist mit {rating} oder höher bewertet.",
    },
    Message {
        english: "1 image remains loaded in this folder.",
        spanish: "Una imagen sigue cargada en esta carpeta.",
        french: "Une image reste chargée dans ce dossier.",
        german: "Ein Bild bleibt in diesem Ordner geladen.",
    },
    Message {
        english: "{count} images remain loaded in this folder.",
        spanish: "{count} imágenes siguen cargadas en esta carpeta.",
        french: "{count} images restent chargées dans ce dossier.",
        german: "{count} Bilder bleiben in diesem Ordner geladen.",
    },
    Message {
        english: "Show all images",
        spanish: "Mostrar todas las imágenes",
        french: "Afficher toutes les images",
        german: "Alle Bilder anzeigen",
    },
    Message {
        english: "Esc or Left/Right also shows all images",
        spanish: "Esc o Left/Right también muestra todas las imágenes",
        french: "Esc ou Left/Right affiche aussi toutes les images",
        german: "Esc oder Left/Right zeigt ebenfalls alle Bilder an",
    },
    Message {
        english: "No images match rating filter",
        spanish: "Ninguna imagen coincide con el filtro de valoración",
        french: "Aucune image ne correspond au filtre de note",
        german: "Kein Bild entspricht dem Bewertungsfilter",
    },
    Message {
        english: "Spot heal is unavailable for images larger than the GPU texture limit",
        spanish: "La corrección puntual no está disponible para imágenes que superan el límite de texturas de la GPU",
        french: "La correction ponctuelle n’est pas disponible pour les images dépassant la limite de texture du GPU",
        german: "Die Bereichsreparatur ist für Bilder über dem GPU-Texturlimit nicht verfügbar",
    },
    Message {
        english: "Done",
        spanish: "Listo",
        french: "Terminé",
        german: "Fertig",
    },
    Message {
        english: "Paint over a small blemish, then release to repair it.",
        spanish: "Pinte sobre una pequeña imperfección y suelte para corregirla.",
        french: "Peignez sur une petite imperfection, puis relâchez pour la corriger.",
        german: "Übermalen Sie einen kleinen Makel und lassen Sie dann los, um ihn zu reparieren.",
    },
    Message {
        english: "Brush radius",
        spanish: "Radio del pincel",
        french: "Rayon du pinceau",
        german: "Pinselradius",
    },
    Message {
        english: "Feather",
        spanish: "Difuminado",
        french: "Contour progressif",
        german: "Randweichheit",
    },
    Message {
        english: "Softens the repair edge outward from the painted area",
        spanish: "Suaviza el borde de la corrección hacia fuera del área pintada",
        french: "Adoucit le bord de la correction vers l’extérieur de la zone peinte",
        german: "Macht den Reparaturrand von der bemalten Fläche nach außen weicher",
    },
    Message {
        english: "Refresh source",
        spanish: "Actualizar origen",
        french: "Actualiser la source",
        german: "Quelle aktualisieren",
    },
    Message {
        english: "Source {index} of {count}",
        spanish: "Origen {index} de {count}",
        french: "Source {index} sur {count}",
        german: "Quelle {index} von {count}",
    },
    Message {
        english: "Try the next ranked clean source patch",
        spanish: "Probar la siguiente zona de origen limpia de la clasificación",
        french: "Essayer le prochain échantillon source propre du classement",
        german: "Nächsten sauberen Quellbereich der Rangfolge versuchen",
    },
    Message {
        english: "Repairing in memory...",
        spanish: "Corrigiendo en memoria...",
        french: "Correction en mémoire...",
        german: "Reparatur im Speicher...",
    },
    Message {
        english: "The original file stays untouched. Use Save As to keep the edit.",
        spanish: "El archivo original no se modifica. Use Guardar como para conservar la edición.",
        french: "Le fichier d’origine reste intact. Utilisez Enregistrer sous pour conserver la modification.",
        german: "Die Originaldatei bleibt unverändert. Verwenden Sie Speichern unter, um die Bearbeitung zu behalten.",
    },
    Message {
        english: "Folder previews  {index} of {total}",
        spanish: "Vistas previas de carpeta  {index} de {total}",
        french: "Aperçus du dossier  {index} sur {total}",
        german: "Ordnervorschauen  {index} von {total}",
    },
    Message {
        english: "Swap",
        spanish: "Intercambiar",
        french: "Permuter",
        german: "Tauschen",
    },
    Message {
        english: "Swap the crop between landscape and portrait",
        spanish: "Alternar el recorte entre horizontal y vertical",
        french: "Basculer le recadrage entre paysage et portrait",
        german: "Zuschnitt zwischen Quer- und Hochformat wechseln",
    },
    Message {
        english: "Arrows move  |  Shift+Arrows resize  |  Ctrl fine-tunes",
        spanish: "Flechas mueven  |  Shift+Flechas redimensionan  |  Ctrl ajusta con precisión",
        french: "Flèches déplacent  |  Shift+Flèches redimensionnent  |  Ctrl affine",
        german: "Pfeiltasten verschieben  |  Shift+Pfeiltasten ändern die Größe  |  Ctrl verfeinert",
    },
    Message {
        english: "Drag to redraw  |  X swaps aspect  |  Enter applies  |  Esc cancels",
        spanish: "Arrastre para redibujar  |  X intercambia la proporción  |  Enter aplica  |  Esc cancela",
        french: "Faites glisser pour redessiner  |  X permute le format  |  Enter applique  |  Esc annule",
        german: "Ziehen zum Neuzeichnen  |  X tauscht das Format  |  Enter wendet an  |  Esc bricht ab",
    },
    Message {
        english: "Aspect: {ratio}",
        spanish: "Proporción: {ratio}",
        french: "Format : {ratio}",
        german: "Seitenverhältnis: {ratio}",
    },
    Message {
        english: "Free",
        spanish: "Libre",
        french: "Libre",
        german: "Frei",
    },
    Message {
        english: "Original",
        spanish: "Original",
        french: "Original",
        german: "Original",
    },
    Message {
        english: "1:1  Square",
        spanish: "1:1  Cuadrado",
        french: "1:1  Carré",
        german: "1:1  Quadrat",
    },
    Message {
        english: "Landscape",
        spanish: "Horizontal",
        french: "Paysage",
        german: "Querformat",
    },
    Message {
        english: "Portrait",
        spanish: "Vertical",
        french: "Portrait",
        german: "Hochformat",
    },
    Message {
        english: "Custom ratio",
        spanish: "Proporción personalizada",
        french: "Format personnalisé",
        german: "Eigenes Seitenverhältnis",
    },
    Message {
        english: "Custom ratio width",
        spanish: "Ancho de la proporción personalizada",
        french: "Largeur du format personnalisé",
        german: "Breite des eigenen Seitenverhältnisses",
    },
    Message {
        english: "Custom ratio height",
        spanish: "Alto de la proporción personalizada",
        french: "Hauteur du format personnalisé",
        german: "Höhe des eigenen Seitenverhältnisses",
    },
    Message {
        english: "Use",
        spanish: "Usar",
        french: "Utiliser",
        german: "Verwenden",
    },
    Message {
        english: "Crop selection: {width} by {height} output pixels, source starts at x {x}, y {y}. Drag inside to move. Arrow keys move; Shift plus Arrow keys resize.",
        spanish: "Selección de recorte: {width} por {height} píxeles de salida; el origen empieza en x {x}, y {y}. Arrastre dentro para mover. Las flechas mueven; Shift más las flechas redimensionan.",
        french: "Sélection du recadrage : {width} sur {height} pixels en sortie ; la source commence en x {x}, y {y}. Faites glisser à l’intérieur pour déplacer. Les flèches déplacent ; Shift et les flèches redimensionnent.",
        german: "Zuschnittauswahl: {width} mal {height} Ausgabepixel, Quelle beginnt bei x {x}, y {y}. Zum Verschieben innen ziehen. Pfeiltasten verschieben; Shift plus Pfeiltasten ändern die Größe.",
    },
    Message {
        english: "Crop selection: {width} by {height} output pixels, source starts at x {x}, y {y}. Arrow keys move; Shift plus Arrow keys resize.",
        spanish: "Selección de recorte: {width} por {height} píxeles de salida; el origen empieza en x {x}, y {y}. Las flechas mueven; Shift más las flechas redimensionan.",
        french: "Sélection du recadrage : {width} sur {height} pixels en sortie ; la source commence en x {x}, y {y}. Les flèches déplacent ; Shift et les flèches redimensionnent.",
        german: "Zuschnittauswahl: {width} mal {height} Ausgabepixel, Quelle beginnt bei x {x}, y {y}. Pfeiltasten verschieben; Shift plus Pfeiltasten ändern die Größe.",
    },
    Message {
        english: "Resize crop from top left",
        spanish: "Redimensionar el recorte desde la esquina superior izquierda",
        french: "Redimensionner le recadrage depuis le coin supérieur gauche",
        german: "Zuschnittgröße von oben links ändern",
    },
    Message {
        english: "Resize crop from top",
        spanish: "Redimensionar el recorte desde arriba",
        french: "Redimensionner le recadrage depuis le haut",
        german: "Zuschnittgröße von oben ändern",
    },
    Message {
        english: "Resize crop from top right",
        spanish: "Redimensionar el recorte desde la esquina superior derecha",
        french: "Redimensionner le recadrage depuis le coin supérieur droit",
        german: "Zuschnittgröße von oben rechts ändern",
    },
    Message {
        english: "Resize crop from right",
        spanish: "Redimensionar el recorte desde la derecha",
        french: "Redimensionner le recadrage depuis la droite",
        german: "Zuschnittgröße von rechts ändern",
    },
    Message {
        english: "Resize crop from bottom right",
        spanish: "Redimensionar el recorte desde la esquina inferior derecha",
        french: "Redimensionner le recadrage depuis le coin inférieur droit",
        german: "Zuschnittgröße von unten rechts ändern",
    },
    Message {
        english: "Resize crop from bottom",
        spanish: "Redimensionar el recorte desde abajo",
        french: "Redimensionner le recadrage depuis le bas",
        german: "Zuschnittgröße von unten ändern",
    },
    Message {
        english: "Resize crop from bottom left",
        spanish: "Redimensionar el recorte desde la esquina inferior izquierda",
        french: "Redimensionner le recadrage depuis le coin inférieur gauche",
        german: "Zuschnittgröße von unten links ändern",
    },
    Message {
        english: "Resize crop from left",
        spanish: "Redimensionar el recorte desde la izquierda",
        french: "Redimensionner le recadrage depuis la gauche",
        german: "Zuschnittgröße von links ändern",
    },
    Message {
        english: "Folder previews",
        spanish: "Vistas previas de carpeta",
        french: "Aperçus du dossier",
        german: "Ordnervorschauen",
    },
    Message {
        english: "Light",
        spanish: "Claro",
        french: "Clair",
        german: "Hell",
    },
    Message {
        english: "Dark",
        spanish: "Oscuro",
        french: "Sombre",
        german: "Dunkel",
    },
    Message {
        english: "Console",
        spanish: "Consola",
        french: "Console",
        german: "Konsole",
    },
    Message {
        english: "Appearance: {name}",
        spanish: "Apariencia: {name}",
        french: "Apparence : {name}",
        german: "Erscheinungsbild: {name}",
    },
    Message {
        english: "Follows your operating system. Currently {mode}.",
        spanish: "Sigue la configuración de su sistema operativo. Actualmente: {mode}.",
        french: "Suit votre système d’exploitation. Actuellement : {mode}.",
        german: "Folgt Ihrem Betriebssystem. Derzeit {mode}.",
    },
    Message {
        english: "Follows your operating system's Light or Dark setting.",
        spanish: "Sigue la configuración Claro u Oscuro del sistema operativo.",
        french: "Suit le réglage Clair ou Sombre de votre système d’exploitation.",
        german: "Folgt der Einstellung Hell oder Dunkel Ihres Betriebssystems.",
    },
    Message {
        english: "Bright neutral chrome, light window frame, soft-white canvas.",
        spanish: "Interfaz neutra y luminosa, marco de ventana claro, lienzo blanco suave.",
        french: "Interface neutre et lumineuse, cadre de fenêtre clair, canevas blanc doux.",
        german: "Helle, neutrale Oberfläche, heller Fensterrahmen, weiche weiße Fläche.",
    },
    Message {
        english: "Low-glare charcoal chrome, dark window frame, deep-ink canvas.",
        spanish: "Interfaz antracita sin deslumbramientos, marco de ventana oscuro, lienzo de tinta profunda.",
        french: "Interface anthracite peu éblouissante, cadre de fenêtre sombre, canevas encre profonde.",
        german: "Blendarme anthrazitfarbene Oberfläche, dunkler Fensterrahmen, tintenschwarze Fläche.",
    },
    Message {
        english: "Green-screen look, near-black canvas, phosphor-green chrome, monospaced type.",
        spanish: "Aspecto de pantalla verde, lienzo casi negro, interfaz verde fósforo, tipografía monoespaciada.",
        french: "Aspect écran vert, canevas presque noir, interface vert phosphore, police à chasse fixe.",
        german: "Grünbildschirm-Optik, fast schwarze Fläche, phosphorgrüne Oberfläche, Festbreitenschrift.",
    },
    Message {
        english: "Photo {position} of {total} in the active folder view, selected",
        spanish: "Foto {position} de {total} en la vista de carpeta activa, seleccionada",
        french: "Photo {position} sur {total} dans la vue du dossier active, sélectionnée",
        german: "Foto {position} von {total} in der aktiven Ordneransicht, ausgewählt",
    },
    Message {
        english: "Page {index} of {count}",
        spanish: "Página {index} de {count}",
        french: "Page {index} sur {count}",
        german: "Seite {index} von {count}",
    },
    Message {
        english: "Icon {index} of {count}",
        spanish: "Icono {index} de {count}",
        french: "Icône {index} sur {count}",
        german: "Symbol {index} von {count}",
    },
    Message {
        english: "{position}, {width} by {height}",
        spanish: "{position}, {width} por {height}",
        french: "{position}, {width} sur {height}",
        german: "{position}, {width} mal {height}",
    },
    Message {
        english: "{index} of {total}",
        spanish: "{index} de {total}",
        french: "{index} sur {total}",
        german: "{index} von {total}",
    },
    Message {
        english: "image {position}: {name}",
        spanish: "imagen {position}: {name}",
        french: "image {position} : {name}",
        german: "Bild {position}: {name}",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Interface literals reach the catalog through `tr!`, never directly.
    ///
    /// The lookup falls back to the English source for an unknown key, so a
    /// bare literal would render English in Spanish, French, and German without
    /// failing anything. `tr!` turns that into a build error, and this keeps a
    /// later change from quietly stepping around it.
    #[test]
    fn interface_literals_bind_to_the_catalog() {
        // Built at run time so this test does not match its own source.
        let bare = [".text", ".localize", ".fill", "::from_translated_seam"]
            .map(|call| format!("{call}(\""));
        let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&source_dir).expect("read src") {
            let path = entry.expect("source entry").path();
            if path.extension().is_none_or(|extension| extension != "rs")
                || path.file_name().is_some_and(|name| name == "locale.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            for (number, line) in source.lines().enumerate() {
                if bare.iter().any(|pattern| line.contains(pattern.as_str())) {
                    offenders.push(format!("{}:{}", path.display(), number + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "pass interface literals through tr! so a missing catalog entry fails the build: {offenders:?}"
        );
    }

    /// Toast copy that reaches `Language::localize` as a constant or seam value
    /// rather than a `tr!` literal, so the build cannot check it.
    #[test]
    fn toast_sources_passed_by_value_are_cataloged() {
        use crate::entry_state::{FolderScanDisposition as Scan, folder_scan_user_message};
        let folder_scan = [
            Scan::Discard,
            Scan::InstallScanAt(0),
            Scan::InstallSelectedOnly,
            Scan::InstallScanFirstAfterSelectedMissing,
            Scan::SelectedMissing,
            Scan::SelectedMissingLimitExceeded,
            Scan::SelectedMissingScanFailed,
            Scan::InstallSelectedOnlyLimitExceeded,
            Scan::InstallSelectedOnlyScanFailed,
            Scan::OpenFolderEmpty,
            Scan::OpenFolderLimitExceeded,
            Scan::OpenFolderFailed,
            Scan::OpenFolderFirst,
        ]
        .into_iter()
        .filter_map(folder_scan_user_message);
        let constants = [
            crate::session::MISSING_IMAGE_STATUS,
            crate::session::FOREGROUND_EXECUTOR_LOSS_STATUS,
            crate::chrome::RATING_RECOVERY_STATUS,
            crate::chrome::RATING_DISCOVERY_WRITE_STATUS,
            crate::chrome::SAVE_RECOVERY_STATUS,
            crate::crop_state::PREVIEW_RECOVERY_STATUS,
            crate::pages::edit_blocks_page_step_copy(),
            crate::theme::appearance_save_failure_message(),
            crate::folder_sort_preference::save_failure_message(),
            save_failure_message(),
        ];
        let mut checked = 0;
        for source in folder_scan.chain(constants) {
            assert!(is_cataloged(source), "uncataloged toast source: {source}");
            for language in [Language::Spanish, Language::French, Language::German] {
                assert_ne!(
                    language.text(source),
                    source,
                    "{language:?} copies English: {source}"
                );
            }
            checked += 1;
        }
        assert_eq!(checked, 19);
    }

    #[test]
    fn fill_substitutes_translated_placeholders_once() {
        assert_eq!(
            Language::French
                .fill(tr!("Save failed: {error}"), &[("error", "disk full")])
                .as_str(),
            "Échec de l’enregistrement : disk full"
        );
        assert_eq!(
            Language::English
                .fill(tr!("Save failed: {error}"), &[("error", "{error} {count}")])
                .as_str(),
            "Save failed: {error} {count}",
            "a substituted value is inserted verbatim, never rescanned"
        );
        assert_eq!(
            Language::English
                .fill(tr!("Save failed: {error}"), &[])
                .as_str(),
            "Save failed: {error}",
            "a missing value stays visible instead of vanishing"
        );
        assert_eq!(
            Language::German.localize(tr!("Close")).into_string(),
            "Schließen"
        );
        assert_eq!(
            Language::English
                .fill(tr!("Save failed: {error}"), &[("error", "} { }{ {")])
                .as_str(),
            "Save failed: } { }{ {",
            "unpaired braces in a value neither panic nor change"
        );
    }

    #[test]
    fn uncataloged_source_is_reported_rather_than_translated() {
        // The same scan `tr!` performs at build time, run here so its comparison
        // is exercised rather than only evaluated by the compiler.
        assert_cataloged("Close");
        assert!(same_source("Close", "Close"));
        assert!(!same_source("Close", "Closed"));
        assert!(!same_source("Close", "Clone"));

        assert!(is_cataloged("Open File..."));
        assert!(!is_cataloged("Open File...."));
        assert_eq!(
            Language::German.text("not in the catalog"),
            "not in the catalog"
        );
        assert_eq!(Language::German.text(tr!("Close")), "Schließen");
    }

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

    fn placeholders(text: &str) -> Vec<&str> {
        let mut names: Vec<&str> = text
            .split('{')
            .skip(1)
            .filter_map(|tail| tail.split_once('}').map(|(name, _)| name))
            .collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn every_translation_keeps_its_source_placeholders_and_is_unique() {
        let mut sources = std::collections::HashSet::new();
        for message in MESSAGES {
            assert!(
                sources.insert(message.english),
                "duplicate catalog source: {}",
                message.english
            );
            let expected = placeholders(message.english);
            for translated in [message.spanish, message.french, message.german] {
                assert_eq!(
                    placeholders(translated),
                    expected,
                    "placeholder drift in {translated:?} for {:?}",
                    message.english
                );
            }
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
