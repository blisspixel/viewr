use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

use crate::decode::LoadedImage;

pub(crate) const MISSING_IMAGE_STATUS: &str = "The selected image is no longer available";
pub(crate) const FOREGROUND_EXECUTOR_LOSS_STATUS: &str = "The image decoder stopped unexpectedly";

/// Decode failure facts captured by the foreground worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ForegroundLoadFailure {
    /// The source was absent when the worker inspected it after failure.
    MissingCandidate(String),
    /// The failure had no evidence that the source was absent.
    Other(String),
}

/// Event-loop disposition after rechecking a foreground load failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ForegroundLoadFailureDisposition {
    /// Remove the stale selection and recover folder navigation.
    MissingSelection,
    /// Report the ordinary decode or source error.
    Other(String),
}

/// Work needed when the user retries the selected image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForegroundRetryPlan {
    LoadSelected,
    LoadAndScanFolder,
}

/// Resolve a possible missing-source failure only after an event-loop recheck.
///
/// A path already being presented belongs to file-coherence handling, which
/// intentionally retains the last good frame when its source disappears.
#[must_use]
pub(crate) fn resolve_foreground_load_failure(
    failure: ForegroundLoadFailure,
    definitely_missing_now: bool,
    selected_is_presented: bool,
) -> ForegroundLoadFailureDisposition {
    match failure {
        ForegroundLoadFailure::MissingCandidate(_)
            if definitely_missing_now && !selected_is_presented =>
        {
            ForegroundLoadFailureDisposition::MissingSelection
        }
        ForegroundLoadFailure::MissingCandidate(error) | ForegroundLoadFailure::Other(error) => {
            ForegroundLoadFailureDisposition::Other(error)
        }
    }
}

#[must_use]
pub(crate) const fn foreground_retry_plan(selected_missing: bool) -> ForegroundRetryPlan {
    if selected_missing {
        ForegroundRetryPlan::LoadAndScanFolder
    } else {
        ForegroundRetryPlan::LoadSelected
    }
}

/// Selected, loading, and presented state for one viewer session.
pub struct Session {
    /// Path most recently requested by the user.
    pub selected_path: Option<PathBuf>,
    /// Path whose pixels are currently rendered on screen.
    pub presented_path: Option<PathBuf>,
    /// Error from the most recent load attempt.
    pub load_error: Option<String>,
    /// Raw decoder or I/O line for Image Information. Absent for status text.
    pub decoder_detail: Option<String>,
    /// Whether Retry can help the current load error.
    pub load_retry_allowed: bool,
    /// Whether the selected, not-yet-presented source was confirmed absent.
    pub selected_missing: bool,
    /// Generation counter used to invalidate stale asynchronous work.
    pub generation: Arc<AtomicU64>,
    /// Receiver for the active asynchronous image decode, if any.
    pub(crate) receiver: Option<Receiver<(PathBuf, Result<LoadedImage, ForegroundLoadFailure>)>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            selected_path: None,
            presented_path: None,
            load_error: None,
            decoder_detail: None,
            load_retry_allowed: true,
            selected_missing: false,
            generation: Arc::new(AtomicU64::new(0)),
            receiver: None,
        }
    }
}

impl Session {
    /// Cancel any in-flight load and invalidate work holding the old generation.
    pub fn cancel_pending_load(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.receiver = None;
        self.clear_load_error();
        self.selected_missing = false;
    }

    /// Clear the previous load error before scheduling a replacement.
    pub fn prepare_for_load(&mut self, preserve_missing_recovery: bool) {
        self.clear_load_error();
        self.selected_missing = preserve_missing_recovery;
    }

    /// Record a successfully presented path and finish the active load.
    pub fn set_presented(&mut self, path: PathBuf) {
        self.presented_path = Some(path);
        self.receiver = None;
        self.clear_load_error();
        self.selected_missing = false;
    }

    /// Drop the card error, its decoder line, and restore Retry for the next failure.
    pub fn clear_load_error(&mut self) {
        self.load_error = None;
        self.decoder_detail = None;
        self.load_retry_allowed = true;
    }

    /// Show a status sentence that is not a classified decode failure.
    pub fn note_status(&mut self, status: impl Into<String>) {
        self.load_error = Some(status.into());
        self.decoder_detail = None;
        self.load_retry_allowed = true;
    }

    /// Show a classified decode failure and keep the raw line for Image Information.
    ///
    /// An empty detail means Image Information has nothing further to show.
    pub fn note_decode_failure(
        &mut self,
        sentence: impl Into<String>,
        detail: impl Into<String>,
        allow_retry: bool,
    ) {
        self.load_error = Some(sentence.into());
        let detail = detail.into();
        let detail = detail.trim();
        self.decoder_detail = if detail.is_empty() {
            None
        } else {
            Some(detail.to_owned())
        };
        self.load_retry_allowed = allow_retry;
    }

    /// Record a confirmed missing selection while retaining any presented frame.
    /// `status` is `MISSING_IMAGE_STATUS` already resolved for the interface
    /// language, because this state is shown as-is on the empty-state card.
    pub fn set_selected_missing(&mut self, status: &str) {
        self.receiver = None;
        self.note_status(status);
        self.selected_missing = true;
    }

    /// Return whether an image decode result is still pending.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.receiver.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn default_session_has_no_selected_or_pending_image() {
        let session = Session::default();

        assert!(session.selected_path.is_none());
        assert!(session.presented_path.is_none());
        assert!(session.load_error.is_none());
        assert!(!session.selected_missing);
        assert!(!session.is_loading());
        assert_eq!(session.generation.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cancellation_invalidates_work_and_clears_transient_state() {
        let (_sender, receiver) = mpsc::channel();
        let mut session = Session {
            load_error: Some("old failure".into()),
            decoder_detail: Some("format error".into()),
            load_retry_allowed: false,
            receiver: Some(receiver),
            ..Session::default()
        };

        session.cancel_pending_load();

        assert_eq!(session.generation.load(Ordering::Acquire), 1);
        assert!(!session.is_loading());
        assert!(session.load_error.is_none());
        assert!(session.decoder_detail.is_none());
        assert!(session.load_retry_allowed);
        assert!(!session.selected_missing);
    }

    #[test]
    fn successful_presentation_finishes_load_and_clears_error() {
        let (_sender, receiver) = mpsc::channel();
        let mut session = Session {
            load_error: Some("old failure".into()),
            decoder_detail: Some("format error".into()),
            load_retry_allowed: false,
            receiver: Some(receiver),
            ..Session::default()
        };

        session.set_presented(PathBuf::from("selected.png"));

        assert_eq!(session.presented_path, Some(PathBuf::from("selected.png")));
        assert!(!session.is_loading());
        assert!(session.load_error.is_none());
        assert!(session.decoder_detail.is_none());
        assert!(session.load_retry_allowed);
        assert!(!session.selected_missing);
    }

    #[test]
    fn preparing_replacement_clears_only_the_error() {
        let (_sender, receiver) = mpsc::channel();
        let mut session = Session {
            selected_path: Some(PathBuf::from("selected.png")),
            load_error: Some("old failure".into()),
            decoder_detail: Some("format error".into()),
            load_retry_allowed: false,
            selected_missing: true,
            receiver: Some(receiver),
            ..Session::default()
        };

        session.prepare_for_load(false);

        assert_eq!(session.selected_path, Some(PathBuf::from("selected.png")));
        assert!(session.is_loading());
        assert!(session.load_error.is_none());
        assert!(session.decoder_detail.is_none());
        assert!(session.load_retry_allowed);
        assert!(!session.selected_missing);
        assert_eq!(session.generation.load(Ordering::Acquire), 0);
    }

    #[test]
    fn missing_candidate_requires_two_absence_checks_and_an_unpresented_selection() {
        let failure = ForegroundLoadFailure::MissingCandidate("source disappeared".into());
        assert_eq!(
            resolve_foreground_load_failure(failure.clone(), true, false),
            ForegroundLoadFailureDisposition::MissingSelection
        );
        assert_eq!(
            resolve_foreground_load_failure(failure.clone(), false, false),
            ForegroundLoadFailureDisposition::Other("source disappeared".into())
        );
        assert_eq!(
            resolve_foreground_load_failure(failure, true, true),
            ForegroundLoadFailureDisposition::Other("source disappeared".into())
        );
        assert_eq!(
            resolve_foreground_load_failure(
                ForegroundLoadFailure::Other("decode failed".into()),
                true,
                false,
            ),
            ForegroundLoadFailureDisposition::Other("decode failed".into())
        );
    }

    #[test]
    fn missing_selection_status_and_retry_plan_are_explicit() {
        let mut session = Session::default();
        session.set_selected_missing(MISSING_IMAGE_STATUS);

        assert!(session.selected_missing);
        assert_eq!(session.load_error.as_deref(), Some(MISSING_IMAGE_STATUS));
        assert!(session.decoder_detail.is_none());
        assert!(session.load_retry_allowed);
        assert_eq!(
            foreground_retry_plan(session.selected_missing),
            ForegroundRetryPlan::LoadAndScanFolder
        );
        session.prepare_for_load(true);
        assert!(session.selected_missing);
        assert!(session.load_error.is_none());
        assert_eq!(
            foreground_retry_plan(session.selected_missing),
            ForegroundRetryPlan::LoadAndScanFolder
        );

        session.note_decode_failure("The file is damaged or incomplete.", "   ", false);
        assert_eq!(
            session.load_error.as_deref(),
            Some("The file is damaged or incomplete.")
        );
        assert!(session.decoder_detail.is_none());
        assert!(!session.load_retry_allowed);
        session.note_decode_failure(
            "viewr doesn't have permission to read this file.",
            "Access is denied. (os error 5)",
            true,
        );
        assert_eq!(
            session.decoder_detail.as_deref(),
            Some("Access is denied. (os error 5)")
        );
        assert!(session.load_retry_allowed);
        session.note_status("The image decoder stopped unexpectedly");
        assert_eq!(
            session.load_error.as_deref(),
            Some("The image decoder stopped unexpectedly")
        );
        assert!(session.decoder_detail.is_none());
        assert!(session.load_retry_allowed);

        session.prepare_for_load(false);
        assert!(!session.selected_missing);
        assert_eq!(
            foreground_retry_plan(session.selected_missing),
            ForegroundRetryPlan::LoadSelected
        );
    }
}
