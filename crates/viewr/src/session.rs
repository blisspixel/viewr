
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use crate::decode::DecodedImage;

pub struct Session {
    pub selected_path: Option<PathBuf>,
    pub presented_path: Option<PathBuf>,
    pub load_error: Option<String>,
    pub generation: Arc<AtomicU64>,
    pub receiver: Option<Receiver<(PathBuf, Result<DecodedImage, String>)>>,
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
    pub fn cancel_pending_load(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.receiver = None;
        self.load_error = None;
    }

    pub fn prepare_for_load(&mut self) {
        self.load_error = None;
    }

    pub fn set_presented(&mut self, path: PathBuf) {
        self.presented_path = Some(path);
        self.load_error = None;
    }

    pub fn is_loading(&self) -> bool {
        self.receiver.is_some()
    }
}
