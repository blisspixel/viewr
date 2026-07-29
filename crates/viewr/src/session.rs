use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

use crate::decode::LoadedImage;

/// Selected, loading, and presented state for one viewer session.
pub struct Session {
    /// Path most recently requested by the user.
    pub selected_path: Option<PathBuf>,
    /// Path whose pixels are currently rendered on screen.
    pub presented_path: Option<PathBuf>,
    /// Error from the most recent load attempt.
    pub load_error: Option<String>,
    /// Generation counter used to invalidate stale asynchronous work.
    pub generation: Arc<AtomicU64>,
    /// Receiver for the active asynchronous image decode, if any.
    pub(crate) receiver: Option<Receiver<(PathBuf, Result<LoadedImage, String>)>>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            selected_path: None,
            presented_path: None,
            load_error: None,
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
        self.load_error = None;
    }

    /// Clear the previous load error before scheduling a replacement.
    pub fn prepare_for_load(&mut self) {
        self.load_error = None;
    }

    /// Record a successfully presented path and finish the active load.
    pub fn set_presented(&mut self, path: PathBuf) {
        self.presented_path = Some(path);
        self.receiver = None;
        self.load_error = None;
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
        assert!(!session.is_loading());
        assert_eq!(session.generation.load(Ordering::Acquire), 0);
    }

    #[test]
    fn cancellation_invalidates_work_and_clears_transient_state() {
        let (_sender, receiver) = mpsc::channel();
        let mut session = Session {
            load_error: Some("old failure".into()),
            receiver: Some(receiver),
            ..Session::default()
        };

        session.cancel_pending_load();

        assert_eq!(session.generation.load(Ordering::Acquire), 1);
        assert!(!session.is_loading());
        assert!(session.load_error.is_none());
    }

    #[test]
    fn successful_presentation_finishes_load_and_clears_error() {
        let (_sender, receiver) = mpsc::channel();
        let mut session = Session {
            load_error: Some("old failure".into()),
            receiver: Some(receiver),
            ..Session::default()
        };

        session.set_presented(PathBuf::from("selected.png"));

        assert_eq!(session.presented_path, Some(PathBuf::from("selected.png")));
        assert!(!session.is_loading());
        assert!(session.load_error.is_none());
    }

    #[test]
    fn preparing_replacement_clears_only_the_error() {
        let (_sender, receiver) = mpsc::channel();
        let mut session = Session {
            selected_path: Some(PathBuf::from("selected.png")),
            load_error: Some("old failure".into()),
            receiver: Some(receiver),
            ..Session::default()
        };

        session.prepare_for_load();

        assert_eq!(session.selected_path, Some(PathBuf::from("selected.png")));
        assert!(session.is_loading());
        assert!(session.load_error.is_none());
        assert_eq!(session.generation.load(Ordering::Acquire), 0);
    }
}
