//! Pure lifecycle policy for displayed and persisted image ratings.
//!
//! The event loop owns workers, accepted sources, paths, playlist mutation,
//! disclosure, and UI dispatch. This module owns only deterministic transitions
//! and terminal dispositions derived from immutable facts.

use crate::chrome::{RATING_DISCOVERY_WRITE_STATUS, RATING_RECOVERY_STATUS};
use crate::ratings::{
    RatingFilter, RatingObservation, RatingState, RatingWriteCapability, RatingWriteError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentedRatingTransition {
    Retain,
    Replace(RatingState),
    Clear,
}

#[must_use]
pub(crate) const fn next_presented_rating(
    current: RatingState,
    transition: PresentedRatingTransition,
) -> RatingState {
    match transition {
        PresentedRatingTransition::Retain => current,
        PresentedRatingTransition::Replace(rating) => rating,
        PresentedRatingTransition::Clear => RatingState::Loading,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RatingRecoveryTransition {
    Retain,
    MarkUnsettled,
    AcceptSource,
}

#[must_use]
pub(crate) const fn next_rating_recovery_state(
    current: bool,
    transition: RatingRecoveryTransition,
) -> bool {
    match transition {
        RatingRecoveryTransition::Retain => current,
        RatingRecoveryTransition::MarkUnsettled => true,
        RatingRecoveryTransition::AcceptSource => false,
    }
}

#[must_use]
pub(crate) const fn rating_recovery_after_presentation(
    loaded_frame: bool,
    accepted_source: bool,
) -> RatingRecoveryTransition {
    if loaded_frame && accepted_source {
        RatingRecoveryTransition::AcceptSource
    } else {
        RatingRecoveryTransition::Retain
    }
}

#[must_use]
pub(crate) const fn rating_recovery_blocker(unsettled: bool) -> Option<&'static str> {
    if unsettled {
        Some(RATING_RECOVERY_STATUS)
    } else {
        None
    }
}

/// A folder-wide discovery result may contain the previous value for the
/// current image. Do not let that stale read race a source mutation and replace
/// the newly committed in-memory value when discovery completes.
#[must_use]
pub(crate) const fn rating_write_discovery_blocker(discovery_active: bool) -> Option<&'static str> {
    if discovery_active {
        Some(RATING_DISCOVERY_WRITE_STATUS)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RatingDiscoveryTransition {
    Apply,
    Start,
    KeepRunning,
    CancelAndApply,
}

#[must_use]
pub(crate) const fn rating_discovery_transition(
    filter: RatingFilter,
    worker_active: bool,
    has_loading_ratings: bool,
) -> RatingDiscoveryTransition {
    if matches!(filter, RatingFilter::All) || !has_loading_ratings {
        if worker_active {
            RatingDiscoveryTransition::CancelAndApply
        } else {
            RatingDiscoveryTransition::Apply
        }
    } else if worker_active {
        RatingDiscoveryTransition::KeepRunning
    } else {
        RatingDiscoveryTransition::Start
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum RatingWriteTerminal<T> {
    Completed(Result<T, RatingWriteError>),
    Disconnected,
}

pub(crate) fn reconcile_rating_write<T>(
    terminal: RatingWriteTerminal<T>,
    worker_panicked: bool,
) -> Result<T, RatingWriteError> {
    if worker_panicked {
        return Err(RatingWriteError::RecoveryFailed);
    }
    match terminal {
        RatingWriteTerminal::Completed(result) => result,
        RatingWriteTerminal::Disconnected => Err(RatingWriteError::RecoveryFailed),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RatingCloseDisposition {
    StayOpen,
    Exit,
}

#[must_use]
pub(crate) const fn rating_close_disposition(
    close_requested: bool,
    terminal_error: Option<RatingWriteError>,
) -> RatingCloseDisposition {
    if close_requested && !matches!(terminal_error, Some(RatingWriteError::RecoveryFailed)) {
        RatingCloseDisposition::Exit
    } else {
        RatingCloseDisposition::StayOpen
    }
}

/// Path-free guidance after the auxiliary details worker endpoint is lost.
#[must_use]
pub(crate) const fn auxiliary_disconnect_message() -> &'static str {
    "Image details, animation, and rating reading stopped unexpectedly. Close and reopen viewr before continuing."
}

/// Rating observation forced after auxiliary endpoint loss.
#[must_use]
pub(crate) const fn rating_after_auxiliary_disconnect() -> RatingObservation {
    RatingObservation {
        state: RatingState::Unreadable,
        capability: RatingWriteCapability::ObservationFailed,
    }
}

/// Path-free user message for a terminal rating write failure.
#[must_use]
pub(crate) const fn rating_write_failure_message(error: RatingWriteError) -> &'static str {
    match error {
        RatingWriteError::ReadOnlyFormat => {
            "This image's rating is read-only in viewr. The file was not changed."
        }
        RatingWriteError::UnsupportedMetadata => {
            "This image has unsupported rating metadata. The file was not changed."
        }
        RatingWriteError::UnreadableMetadata => {
            "viewr could not read this image's rating safely. The file was not changed."
        }
        RatingWriteError::SourceChanged => {
            "The image changed on disk before the rating could be saved. Press F5 to reload, then try again."
        }
        RatingWriteError::PermissionDenied => {
            "Could not save the rating because the image or its folder is read-only. The previous rating is unchanged."
        }
        RatingWriteError::WriteFailed => {
            "Could not save the rating safely. The previous rating is unchanged."
        }
        RatingWriteError::VerificationRestored => {
            "The rating update could not be verified. The original image was restored."
        }
        RatingWriteError::RecoveryFailed => {
            "The rating update could not be verified or restored. Stop editing this image and restore it from a trusted backup."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratings::Rating;

    const WRITE_ERRORS: [RatingWriteError; 8] = [
        RatingWriteError::ReadOnlyFormat,
        RatingWriteError::UnsupportedMetadata,
        RatingWriteError::UnreadableMetadata,
        RatingWriteError::SourceChanged,
        RatingWriteError::PermissionDenied,
        RatingWriteError::WriteFailed,
        RatingWriteError::VerificationRestored,
        RatingWriteError::RecoveryFailed,
    ];

    #[test]
    fn presented_rating_transitions_preserve_last_good_state_until_replacement() {
        let current = RatingState::Rated(Rating::new(4).unwrap());

        assert_eq!(
            next_presented_rating(current, PresentedRatingTransition::Retain),
            current
        );
        assert_eq!(
            next_presented_rating(current, PresentedRatingTransition::Clear),
            RatingState::Loading
        );
        assert_eq!(
            next_presented_rating(
                current,
                PresentedRatingTransition::Replace(RatingState::Unrated)
            ),
            RatingState::Unrated
        );
    }

    #[test]
    fn recovery_transition_exhausts_current_and_requested_states() {
        let cases = [
            (false, RatingRecoveryTransition::Retain, false),
            (true, RatingRecoveryTransition::Retain, true),
            (false, RatingRecoveryTransition::MarkUnsettled, true),
            (true, RatingRecoveryTransition::MarkUnsettled, true),
            (false, RatingRecoveryTransition::AcceptSource, false),
            (true, RatingRecoveryTransition::AcceptSource, false),
        ];

        for (current, transition, expected) in cases {
            assert_eq!(next_rating_recovery_state(current, transition), expected);
        }
    }

    #[test]
    fn rating_write_waits_for_folder_discovery_to_avoid_stale_replacement() {
        assert_eq!(
            rating_write_discovery_blocker(true),
            Some(RATING_DISCOVERY_WRITE_STATUS)
        );
        assert_eq!(rating_write_discovery_blocker(false), None);
    }

    #[test]
    fn only_a_loaded_frame_with_an_accepted_source_clears_recovery() {
        let cases = [
            (false, false, RatingRecoveryTransition::Retain),
            (false, true, RatingRecoveryTransition::Retain),
            (true, false, RatingRecoveryTransition::Retain),
            (true, true, RatingRecoveryTransition::AcceptSource),
        ];

        for (loaded_frame, accepted_source, expected) in cases {
            assert_eq!(
                rating_recovery_after_presentation(loaded_frame, accepted_source),
                expected
            );
        }
    }

    #[test]
    fn recovery_blocker_uses_the_canonical_exact_guidance() {
        assert_eq!(rating_recovery_blocker(false), None);
        assert_eq!(rating_recovery_blocker(true), Some(RATING_RECOVERY_STATUS));
        assert_eq!(
            RATING_RECOVERY_STATUS,
            "Rating update is not settled. Restore this image from a trusted backup, then press F5 to reload."
        );
    }

    #[test]
    fn auxiliary_disconnect_copy_requires_a_restart_without_promising_success() {
        let message = auxiliary_disconnect_message();
        assert!(message.contains("Close and reopen viewr"));
        assert!(message.contains("rating"));
        assert!(!message.contains("recover"));
        assert_eq!(
            rating_after_auxiliary_disconnect(),
            RatingObservation {
                state: RatingState::Unreadable,
                capability: RatingWriteCapability::ObservationFailed,
            }
        );
    }

    #[test]
    fn rating_write_failure_copy_is_exhaustive_and_path_free() {
        for error in WRITE_ERRORS {
            let message = rating_write_failure_message(error);
            assert!(!message.is_empty());
            assert!(!message.contains('\\'));
            assert!(!message.contains('/'));
            assert!(!message.contains('\n'));
        }
        assert_eq!(
            rating_write_failure_message(RatingWriteError::WriteFailed),
            "Could not save the rating safely. The previous rating is unchanged."
        );
    }

    #[test]
    fn discovery_transition_exhausts_filter_worker_and_loading_state() {
        let all_cases = [
            (false, false, RatingDiscoveryTransition::Apply),
            (true, false, RatingDiscoveryTransition::CancelAndApply),
            (false, true, RatingDiscoveryTransition::Apply),
            (true, true, RatingDiscoveryTransition::CancelAndApply),
        ];
        for (worker_active, has_loading_ratings, expected) in all_cases {
            assert_eq!(
                rating_discovery_transition(RatingFilter::All, worker_active, has_loading_ratings),
                expected
            );
        }

        let threshold_cases = [
            (false, false, RatingDiscoveryTransition::Apply),
            (true, false, RatingDiscoveryTransition::CancelAndApply),
            (false, true, RatingDiscoveryTransition::Start),
            (true, true, RatingDiscoveryTransition::KeepRunning),
        ];
        for value in 1..=5 {
            let filter = RatingFilter::AtLeast(Rating::new(value).unwrap());
            for (worker_active, has_loading_ratings, expected) in threshold_cases {
                assert_eq!(
                    rating_discovery_transition(filter, worker_active, has_loading_ratings),
                    expected
                );
            }
        }
    }

    #[test]
    fn terminal_writer_reconciliation_preserves_proven_results_only() {
        assert_eq!(
            reconcile_rating_write(RatingWriteTerminal::Completed(Ok(7_u8)), false),
            Ok(7)
        );
        for error in WRITE_ERRORS {
            assert_eq!(
                reconcile_rating_write::<u8>(RatingWriteTerminal::Completed(Err(error)), false),
                Err(error)
            );
        }
        assert_eq!(
            reconcile_rating_write(RatingWriteTerminal::Completed(Ok(7_u8)), true),
            Err(RatingWriteError::RecoveryFailed)
        );
        assert_eq!(
            reconcile_rating_write::<u8>(
                RatingWriteTerminal::Completed(Err(RatingWriteError::WriteFailed)),
                true
            ),
            Err(RatingWriteError::RecoveryFailed)
        );
        for worker_panicked in [false, true] {
            assert_eq!(
                reconcile_rating_write::<u8>(RatingWriteTerminal::Disconnected, worker_panicked),
                Err(RatingWriteError::RecoveryFailed)
            );
        }
    }

    #[test]
    fn close_disposition_exhausts_requests_and_terminal_results() {
        assert_eq!(
            rating_close_disposition(false, None),
            RatingCloseDisposition::StayOpen
        );
        assert_eq!(
            rating_close_disposition(true, None),
            RatingCloseDisposition::Exit
        );

        let requested_cases = [
            (
                RatingWriteError::ReadOnlyFormat,
                RatingCloseDisposition::Exit,
            ),
            (
                RatingWriteError::UnsupportedMetadata,
                RatingCloseDisposition::Exit,
            ),
            (
                RatingWriteError::UnreadableMetadata,
                RatingCloseDisposition::Exit,
            ),
            (
                RatingWriteError::SourceChanged,
                RatingCloseDisposition::Exit,
            ),
            (
                RatingWriteError::PermissionDenied,
                RatingCloseDisposition::Exit,
            ),
            (RatingWriteError::WriteFailed, RatingCloseDisposition::Exit),
            (
                RatingWriteError::VerificationRestored,
                RatingCloseDisposition::Exit,
            ),
            (
                RatingWriteError::RecoveryFailed,
                RatingCloseDisposition::StayOpen,
            ),
        ];
        for (error, expected) in requested_cases {
            assert_eq!(rating_close_disposition(true, Some(error)), expected);
        }
        for error in WRITE_ERRORS {
            assert_eq!(
                rating_close_disposition(false, Some(error)),
                RatingCloseDisposition::StayOpen
            );
        }
    }
}
