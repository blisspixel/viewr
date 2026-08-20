//! Covered first-run and Help shortcut copy.
//!
//! About, the empty state, and the product-quality matrix quote this catalog
//! instead of a truncated one-line summary.

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
pub(crate) const FIRST_RUN_SCOPE: &str = "Open a file to start, or drop a file or folder. Its folder is browsed when access allows. Open Folder selects it explicitly for this session.";
/// Shown while the first decode of a launch or retry is in progress.
pub(crate) const OPENING_DESCRIPTION: &str = "Decoding locally while the window stays responsive.";
const MAX_EMPTY_ERROR_CHARS: usize = 160;

/// Help catalog. Keys match the event-loop shortcuts, including page step.
pub(crate) const ABOUT_SHORTCUT_GROUPS: &[ShortcutGroup] = &[
    ShortcutGroup {
        heading: "Open",
        items: &[
            ShortcutSpec {
                keys: "O",
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
                keys: "F",
                action: "Fullscreen",
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
#[must_use]
pub(crate) fn empty_state_copy(
    is_opening: bool,
    load_error: Option<&str>,
    selected_file_name: Option<&str>,
) -> EmptyStateCopy {
    let subject = selected_file_name.unwrap_or("image");
    if is_opening {
        return EmptyStateCopy {
            heading: format!("Opening {subject}"),
            description: OPENING_DESCRIPTION.to_owned(),
            show_retry: false,
        };
    }
    if let Some(error) = load_error {
        return EmptyStateCopy {
            heading: format!("Could not open {subject}"),
            description: bound_user_error(error),
            show_retry: true,
        };
    }
    EmptyStateCopy {
        heading: "Open an image".to_owned(),
        description: FIRST_RUN_SCOPE.to_owned(),
        show_retry: false,
    }
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
            "O Open file",
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
        let first = empty_state_copy(false, None, None);
        assert_eq!(first.heading, "Open an image");
        assert_eq!(first.description, FIRST_RUN_SCOPE);
        assert!(first.description.contains("drop a file or folder"));
        assert!(!first.show_retry);

        let opening = empty_state_copy(true, None, Some("night.png"));
        assert_eq!(opening.heading, "Opening night.png");
        assert_eq!(opening.description, OPENING_DESCRIPTION);
        assert!(!opening.show_retry);

        let failed = empty_state_copy(
            false,
            Some("Could not decode: truncated"),
            Some("night.png"),
        );
        assert_eq!(failed.heading, "Could not open night.png");
        assert_eq!(failed.description, "Could not decode: truncated");
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
