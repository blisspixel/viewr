//! Covered first-run and Help shortcut copy.
//!
//! About, the empty state, and the product-quality matrix quote this catalog
//! instead of a truncated one-line summary. Every source string here is an
//! English catalog key, and `every_visible_string_is_cataloged` keeps it that
//! way, so the surface a first-time user reads is translated rather than
//! falling back to English one string at a time.

use crate::locale::Language;

/// How the empty-state card should speak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmptyStateCopy {
    /// Heading: open, opening, or could-not-open.
    pub heading: String,
    /// Scope, busy, or bounded error description.
    pub description: String,
    /// Whether Retry belongs on the card.
    pub show_retry: bool,
}

/// One Help shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutSpec {
    /// Key text. `{primary}` is replaced with Ctrl or Cmd.
    pub keys: &'static str,
    /// What the keys do.
    pub action: &'static str,
}

/// Named group shown in About.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutGroup {
    /// Group heading.
    pub heading: &'static str,
    /// Shortcuts in this group.
    pub items: &'static [ShortcutSpec],
}

/// First-run card copy when no file is selected.
pub(crate) const FIRST_RUN_SCOPE: &str = "Open File, Open Folder, or drop a file or folder. A dropped file also browses its folder when access allows. Open Folder selects the folder for this session.";
/// Shown while the first decode of a launch or retry is in progress.
pub(crate) const OPENING_DESCRIPTION: &str = "Decoding locally while the window stays responsive.";
/// Heading template while the first decode is in progress.
const OPENING_HEADING: &str = "Opening {subject}";
/// Heading template after a failed open.
const FAILED_HEADING: &str = "Could not open {subject}";
/// Heading with nothing selected.
const EMPTY_HEADING: &str = "Open an image";
/// Placeholder replaced with the selected file name after translation.
const SUBJECT_PLACEHOLDER: &str = "{subject}";
/// Heading while opening a file whose name is not known.
const OPENING_UNNAMED_HEADING: &str = "Opening the image";
/// Heading after a failed open of a file whose name is not known.
const FAILED_UNNAMED_HEADING: &str = "Could not open the image";
/// Retry control name after a failed open.
const RETRY_HEADING: &str = "Retry opening {subject}";
/// Retry control name when the file name is not known.
const RETRY_UNNAMED_HEADING: &str = "Retry opening the image";
const MAX_EMPTY_ERROR_CHARS: usize = 160;

/// Help catalog. Keys match the event-loop shortcuts, including page step.
pub(crate) const ABOUT_SHORTCUT_GROUPS: &[ShortcutGroup] = &[
    ShortcutGroup {
        heading: "Open",
        items: &[
            ShortcutSpec {
                keys: "{primary}+O",
                action: "Open file",
            },
            ShortcutSpec {
                keys: "{primary}+Shift+O",
                action: "Open folder",
            },
            ShortcutSpec {
                keys: "{primary}+Shift+S",
                action: "Save As",
            },
        ],
    },
    ShortcutGroup {
        heading: "Browse",
        items: &[
            ShortcutSpec {
                keys: "Left / Right",
                action: "Previous / next image",
            },
            ShortcutSpec {
                keys: "Home / End",
                action: "First / last image",
            },
            ShortcutSpec {
                keys: "Page Up / Page Down",
                action: "Previous / next image or collage group",
            },
            ShortcutSpec {
                keys: "[ / ]",
                action: "Previous / next page or frame",
            },
            ShortcutSpec {
                keys: "F5",
                action: "Reload file",
            },
        ],
    },
    ShortcutGroup {
        heading: "View",
        items: &[
            ShortcutSpec {
                keys: "Space",
                action: "Fit; hold to pan",
            },
            ShortcutSpec {
                keys: "{primary}+0",
                action: "Fit",
            },
            ShortcutSpec {
                keys: "{primary}+1",
                action: "Actual size",
            },
            ShortcutSpec {
                keys: "+ / -",
                action: "Zoom",
            },
            ShortcutSpec {
                keys: "F / F11",
                action: "Fullscreen",
            },
            ShortcutSpec {
                keys: "Up / Shift+G",
                action: "Full-image collage",
            },
            ShortcutSpec {
                keys: "Esc",
                action: "Leave tool, collage, or fullscreen",
            },
            ShortcutSpec {
                keys: "T G I",
                action: "Panels",
            },
        ],
    },
    ShortcutGroup {
        heading: "Edit",
        items: &[
            ShortcutSpec {
                keys: "0-5",
                action: "Clear or set rating",
            },
            ShortcutSpec {
                keys: "C / J",
                action: "Crop / Spot Heal",
            },
            ShortcutSpec {
                keys: "R L H V",
                action: "Rotate and flip",
            },
            ShortcutSpec {
                keys: "Delete",
                action: "Move to Trash",
            },
            ShortcutSpec {
                keys: "U",
                action: "Undo Trash",
            },
        ],
    },
];

/// Replace `{primary}` with the platform modifier name.
#[must_use]
pub(crate) fn format_shortcut_keys(keys: &str, primary: &str) -> String {
    keys.replace("{primary}", primary)
}

/// Empty, opening, or failed-open copy for the startup card.
///
/// The file name is substituted after translation so a language can place the
/// subject where its grammar needs it. Decoder and I/O error text stays in the
/// words the platform produced.
#[must_use]
pub(crate) fn empty_state_copy(
    language: Language,
    is_opening: bool,
    load_error: Option<&str>,
    selected_file_name: Option<&str>,
) -> EmptyStateCopy {
    if is_opening {
        return EmptyStateCopy {
            heading: heading_for(
                language,
                OPENING_HEADING,
                OPENING_UNNAMED_HEADING,
                selected_file_name,
            ),
            description: language.text(OPENING_DESCRIPTION).to_owned(),
            show_retry: false,
        };
    }
    if let Some(error) = load_error {
        return EmptyStateCopy {
            heading: heading_for(
                language,
                FAILED_HEADING,
                FAILED_UNNAMED_HEADING,
                selected_file_name,
            ),
            description: bound_user_error(error),
            show_retry: true,
        };
    }
    EmptyStateCopy {
        heading: language.text(EMPTY_HEADING).to_owned(),
        description: language.text(FIRST_RUN_SCOPE).to_owned(),
        show_retry: false,
    }
}

/// Top-status line for an image that is opening or failed to open.
///
/// The status line and the empty-state card say the same sentence, so they
/// share one owner instead of formatting it twice in two languages.
#[must_use]
pub(crate) fn open_status(
    language: Language,
    is_opening: bool,
    has_error: bool,
    selected_file_name: Option<&str>,
) -> Option<String> {
    if has_error {
        return Some(heading_for(
            language,
            FAILED_HEADING,
            FAILED_UNNAMED_HEADING,
            selected_file_name,
        ));
    }
    is_opening.then(|| {
        heading_for(
            language,
            OPENING_HEADING,
            OPENING_UNNAMED_HEADING,
            selected_file_name,
        )
    })
}

/// Accessible and visible name for the empty-state Retry control.
#[must_use]
pub(crate) fn retry_label(language: Language, selected_file_name: Option<&str>) -> String {
    heading_for(
        language,
        RETRY_HEADING,
        RETRY_UNNAMED_HEADING,
        selected_file_name,
    )
}

fn heading_for(
    language: Language,
    named: &'static str,
    unnamed: &'static str,
    selected_file_name: Option<&str>,
) -> String {
    let Some(name) = selected_file_name else {
        return language.text(unnamed).to_owned();
    };
    language.text(named).replace(SUBJECT_PLACEHOLDER, name)
}

/// Keep decoder and I/O errors from overflowing the empty-state card.
#[must_use]
pub(crate) fn bound_user_error(error: &str) -> String {
    let line = error.lines().next().unwrap_or(error).trim();
    if line.chars().count() <= MAX_EMPTY_ERROR_CHARS {
        return line.to_owned();
    }
    let bounded: String = line.chars().take(MAX_EMPTY_ERROR_CHARS).collect();
    format!("{bounded}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::is_cataloged;

    /// Every string this seam can show has a translation in all four languages.
    ///
    /// The lookup falls back to English for an uncataloged key, so without this
    /// the first surface a new user sees would quietly stay English while the
    /// menus around it changed language.
    #[test]
    fn every_visible_string_is_cataloged() {
        let mut visible: Vec<&'static str> = vec![
            FIRST_RUN_SCOPE,
            OPENING_DESCRIPTION,
            OPENING_HEADING,
            FAILED_HEADING,
            EMPTY_HEADING,
            OPENING_UNNAMED_HEADING,
            FAILED_UNNAMED_HEADING,
            RETRY_HEADING,
            RETRY_UNNAMED_HEADING,
        ];
        for group in ABOUT_SHORTCUT_GROUPS {
            visible.push(group.heading);
            visible.extend(group.items.iter().map(|item| item.action));
        }
        let missing: Vec<_> = visible
            .into_iter()
            .filter(|source| !is_cataloged(source))
            .collect();
        assert!(
            missing.is_empty(),
            "uncataloged first-run copy: {missing:?}"
        );
    }

    #[test]
    fn headings_substitute_the_file_name_after_translation() {
        let opening = empty_state_copy(Language::German, true, None, Some("night.png"));
        assert_eq!(opening.heading, "night.png wird geöffnet");
        assert_eq!(
            opening.description,
            "Lokale Dekodierung, während das Fenster reaktionsfähig bleibt."
        );

        let unnamed = empty_state_copy(Language::Spanish, true, None, None);
        assert_eq!(unnamed.heading, "Abriendo la imagen");

        let failed = empty_state_copy(Language::French, false, Some("truncated"), Some("a.png"));
        assert_eq!(failed.heading, "Impossible d’ouvrir a.png");
        assert_eq!(
            failed.description, "truncated",
            "decoder text stays as produced"
        );
    }

    #[test]
    fn status_line_and_empty_card_say_the_same_sentence() {
        for language in [
            Language::English,
            Language::Spanish,
            Language::French,
            Language::German,
        ] {
            for name in [Some("night.png"), None] {
                assert_eq!(
                    open_status(language, true, false, name).as_deref(),
                    Some(
                        empty_state_copy(language, true, None, name)
                            .heading
                            .as_str()
                    ),
                );
                assert_eq!(
                    open_status(language, false, true, name).as_deref(),
                    Some(
                        empty_state_copy(language, false, Some("failed"), name)
                            .heading
                            .as_str()
                    ),
                );
            }
            assert_eq!(open_status(language, false, false, None), None);
        }
    }

    #[test]
    fn user_facing_copy_names_the_collage_not_the_module() {
        let actions: Vec<_> = ABOUT_SHORTCUT_GROUPS
            .iter()
            .flat_map(|group| group.items.iter())
            .map(|item| item.action)
            .collect();
        assert!(
            !actions.iter().any(|action| action.contains("mosaic")),
            "the user-facing name is collage: {actions:?}"
        );
    }

    #[test]
    fn about_catalog_covers_pages_reload_panels_and_fit() {
        let rendered: Vec<_> = ABOUT_SHORTCUT_GROUPS
            .iter()
            .flat_map(|group| group.items.iter())
            .map(|item| {
                format!(
                    "{} {}",
                    format_shortcut_keys(item.keys, "Ctrl"),
                    item.action
                )
            })
            .collect();
        for expected in [
            "[ / ] Previous / next page or frame",
            "F5 Reload file",
            "T G I Panels",
            "Space Fit; hold to pan",
            "Ctrl+O Open file",
            "Page Up / Page Down Previous / next image or collage group",
            "F / F11 Fullscreen",
            "Up / Shift+G Full-image collage",
            "Esc Leave tool, collage, or fullscreen",
            "Ctrl+Shift+S Save As",
            "Delete Move to Trash",
            "U Undo Trash",
        ] {
            assert!(
                rendered.iter().any(|line| line == expected),
                "missing Help shortcut {expected}: {rendered:?}"
            );
        }
        assert!(!rendered.iter().any(|line| line.contains("A / D")));
        assert_eq!(format_shortcut_keys("{primary}+0", "Cmd"), "Cmd+0");
    }

    #[test]
    fn empty_state_copy_distinguishes_first_run_opening_and_failure() {
        let first = empty_state_copy(Language::English, false, None, None);
        assert_eq!(first.heading, "Open an image");
        assert_eq!(first.description, FIRST_RUN_SCOPE);
        assert!(first.description.contains("drop a file or folder"));
        assert!(!first.show_retry);

        let opening = empty_state_copy(Language::English, true, None, Some("night.png"));
        assert_eq!(opening.heading, "Opening night.png");
        assert_eq!(opening.description, OPENING_DESCRIPTION);
        assert!(!opening.show_retry);

        let failed = empty_state_copy(
            Language::English,
            false,
            Some("truncated"),
            Some("night.png"),
        );
        assert_eq!(failed.heading, "Could not open night.png");
        assert_eq!(failed.description, "truncated");
        assert!(failed.show_retry);
    }

    #[test]
    fn user_errors_keep_one_short_line() {
        let long = "x".repeat(200);
        let bounded = bound_user_error(&format!("{long}\nsecond line"));
        assert_eq!(bounded.chars().count(), MAX_EMPTY_ERROR_CHARS + 3);
        assert!(bounded.ends_with("..."));
        assert!(!bounded.contains('\n'));
        assert_eq!(bound_user_error("  neat  "), "neat");
        let wide = "é".repeat(200);
        let bounded_wide = bound_user_error(&wide);
        assert_eq!(bounded_wide.chars().count(), MAX_EMPTY_ERROR_CHARS + 3);
        assert!(bounded_wide.ends_with("..."));
        assert!(!bounded_wide.contains('\n'));
    }
}
