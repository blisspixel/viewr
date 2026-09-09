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
        let bare = format!(".{}(\"", "text");
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
                if line.contains(&bare) {
                    offenders.push(format!("{}:{}", path.display(), number + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "pass interface literals through tr! so a missing catalog entry fails the build: {offenders:?}"
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
