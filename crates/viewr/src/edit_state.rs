//! Pure canvas-tool policy and recovery copy for edit presentation failures.
//!
//! The event loop owns GPU presentation, history mutation, and source reload.
//! This module owns tool selection and path-free messages derived from failure class.

use crate::heal::PatchPresentationError;
use crate::locale::Language;

/// Pointer interaction on the image canvas. Space temporarily overrides an edit tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CanvasTool {
    Pan,
    Crop,
    SpotHeal,
}

#[must_use]
pub(crate) const fn canvas_tool(
    is_cropping: bool,
    is_healing: bool,
    space_held: bool,
) -> CanvasTool {
    if space_held {
        CanvasTool::Pan
    } else if is_healing {
        CanvasTool::SpotHeal
    } else if is_cropping {
        CanvasTool::Crop
    } else {
        CanvasTool::Pan
    }
}

/// A drag beyond the click tolerance cannot become the first half of a double-click.
#[must_use]
pub(crate) fn within_click_distance(start: (f64, f64), pointer: (f64, f64)) -> bool {
    (pointer.0 - start.0).hypot(pointer.1 - start.1) < 6.0
}

/// Which edit failed to present. Typed so a caller cannot pass an untranslated phrase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditAction {
    Undo,
    Redo,
    SpotHeal,
}

impl EditAction {
    const fn source(self) -> &'static str {
        match self {
            Self::Undo => ACTION_UNDO,
            Self::Redo => ACTION_REDO,
            Self::SpotHeal => ACTION_SPOT_HEAL,
        }
    }
}

const ACTION_UNDO: &str = "Undo";
const ACTION_REDO: &str = "Redo";
const ACTION_SPOT_HEAL: &str = "Spot heal";
const ACTION_PLACEHOLDER: &str = "{action}";
const APPLY_FAILED: &str =
    "{action} could not be applied. The image and edit history are unchanged.";
const PRESENTATION_FAILED: &str =
    "{action} was not applied because the display could not update. Try again.";
const ROLLBACK_RELOADING: &str =
    "{action} failed. Disk source unchanged; reloading it and clearing edit history.";
const ROLLBACK_REOPEN: &str =
    "{action} failed. Disk source unchanged; reopen it. Edit history was cleared.";

#[cfg(test)]
const ALL_COPY: &[&str] = &[
    ACTION_UNDO,
    ACTION_REDO,
    ACTION_SPOT_HEAL,
    APPLY_FAILED,
    PRESENTATION_FAILED,
    ROLLBACK_RELOADING,
    ROLLBACK_REOPEN,
];

/// Fixed, path-free guidance after an edit presentation transaction fails.
#[must_use]
pub(crate) fn edit_transaction_failure_message<E>(
    language: Language,
    action: EditAction,
    error: &PatchPresentationError<E>,
    reloading_source: bool,
) -> String {
    let template = match error {
        PatchPresentationError::Edit(_) => APPLY_FAILED,
        PatchPresentationError::Presentation(_) => PRESENTATION_FAILED,
        PatchPresentationError::Rollback { .. } if reloading_source => ROLLBACK_RELOADING,
        PatchPresentationError::Rollback { .. } => ROLLBACK_REOPEN,
    };
    language
        .text(template)
        .replace(ACTION_PLACEHOLDER, language.text(action.source()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heal::HealError;
    use crate::locale::is_cataloged;

    const EN: Language = Language::English;

    #[test]
    fn the_image_defaults_to_pan_and_edit_tools_require_explicit_activation() {
        assert_eq!(canvas_tool(false, false, false), CanvasTool::Pan);
        assert_eq!(canvas_tool(true, false, false), CanvasTool::Crop);
        assert_eq!(canvas_tool(false, true, false), CanvasTool::SpotHeal);
        for (cropping, healing) in [(false, false), (true, false), (false, true)] {
            assert_eq!(canvas_tool(cropping, healing, true), CanvasTool::Pan);
        }
    }

    #[test]
    fn dragging_does_not_prime_a_double_click() {
        let start = (100.0, 200.0);
        assert!(within_click_distance(start, start));
        assert!(within_click_distance(start, (103.0, 204.0)));
        assert!(!within_click_distance(start, (106.0, 200.0)));
        assert!(!within_click_distance(start, (96.0, 195.0)));
    }

    #[test]
    fn every_edit_message_is_cataloged() {
        let missing: Vec<_> = ALL_COPY
            .iter()
            .copied()
            .filter(|source| !is_cataloged(source))
            .collect();
        assert!(missing.is_empty(), "uncataloged edit copy: {missing:?}");
    }

    #[test]
    fn edit_copy_is_translated_and_substituted() {
        let error = PatchPresentationError::<&str>::Edit(HealError::InvalidImageBuffer);
        let english = edit_transaction_failure_message(EN, EditAction::Undo, &error, false);
        for language in [Language::Spanish, Language::French, Language::German] {
            let translated =
                edit_transaction_failure_message(language, EditAction::Undo, &error, false);
            assert_ne!(translated, english);
            assert!(!translated.contains('{'));
        }
    }

    #[test]
    fn edit_transaction_copy_is_truthful_and_hides_internal_errors() {
        let edit_error = PatchPresentationError::<&str>::Edit(HealError::InvalidImageBuffer);
        let edit_message =
            edit_transaction_failure_message(EN, EditAction::Undo, &edit_error, false);
        assert_eq!(
            edit_message,
            "Undo could not be applied. The image and edit history are unchanged."
        );
        assert!(!edit_message.contains("RGBA"));

        let rollback_error = PatchPresentationError::Rollback {
            presentation: "adapter rejected the update",
            rollback: HealError::InvalidPatch,
        };
        assert_eq!(
            edit_transaction_failure_message(EN, EditAction::SpotHeal, &rollback_error, true),
            "Spot heal failed. Disk source unchanged; reloading it and clearing edit history."
        );
        assert_eq!(
            edit_transaction_failure_message(EN, EditAction::SpotHeal, &rollback_error, false),
            "Spot heal failed. Disk source unchanged; reopen it. Edit history was cleared."
        );
    }

    #[test]
    fn presentation_failure_offers_retry_without_internal_payloads() {
        let secret = "C:\\private\\album\\bad\n\u{202e}.png";
        let error = PatchPresentationError::Presentation(secret);
        let message = edit_transaction_failure_message(EN, EditAction::SpotHeal, &error, false);
        assert!(message.ends_with("Try again."));
        assert!(!message.contains("Spot healed"));
        assert!(!message.contains("private"));
        assert!(!message.contains('\n'));
        assert!(!message.contains('\u{202e}'));
    }
}
