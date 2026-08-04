//! Pure recovery copy for edit presentation failures.
//!
//! The event loop owns GPU presentation, history mutation, and source reload.
//! This module owns only path-free user messages derived from failure class.

use crate::heal::PatchPresentationError;

/// Fixed, path-free guidance after an edit presentation transaction fails.
#[must_use]
pub(crate) fn edit_transaction_failure_message<E>(
    action: &str,
    error: &PatchPresentationError<E>,
    reloading_source: bool,
) -> String {
    match error {
        PatchPresentationError::Edit(_) => {
            format!("{action} could not be applied. The image and edit history are unchanged.")
        }
        PatchPresentationError::Presentation(_) => {
            format!("{action} was not applied because the display could not update. Try again.")
        }
        PatchPresentationError::Rollback { .. } if reloading_source => format!(
            "{action} failed. Disk source unchanged; reloading it and clearing edit history."
        ),
        PatchPresentationError::Rollback { .. } => {
            format!("{action} failed. Disk source unchanged; reopen it. Edit history was cleared.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heal::HealError;

    #[test]
    fn edit_transaction_copy_is_truthful_and_hides_internal_errors() {
        let edit_error = PatchPresentationError::<&str>::Edit(HealError::InvalidImageBuffer);
        let edit_message = edit_transaction_failure_message("Undo", &edit_error, false);
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
            edit_transaction_failure_message("Spot heal", &rollback_error, true),
            "Spot heal failed. Disk source unchanged; reloading it and clearing edit history."
        );
        assert_eq!(
            edit_transaction_failure_message("Spot heal", &rollback_error, false),
            "Spot heal failed. Disk source unchanged; reopen it. Edit history was cleared."
        );
    }

    #[test]
    fn presentation_failure_offers_retry_without_internal_payloads() {
        let secret = "C:\\private\\album\\bad\n\u{202e}.png";
        let error = PatchPresentationError::Presentation(secret);
        let message = edit_transaction_failure_message("Spot heal", &error, false);
        assert!(message.ends_with("Try again."));
        assert!(!message.contains("Spot healed"));
        assert!(!message.contains("private"));
        assert!(!message.contains('\n'));
        assert!(!message.contains('\u{202e}'));
    }
}
