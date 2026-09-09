//! Pure recovery copy for edit presentation failures.
//!
//! The event loop owns GPU presentation, history mutation, and source reload.
//! This module owns only path-free user messages derived from failure class.

use crate::heal::PatchPresentationError;
use crate::locale::Language;

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
