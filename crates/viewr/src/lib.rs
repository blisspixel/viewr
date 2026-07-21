//! viewr: a photo viewer that just shows your photos.
//!
//! This crate hosts both the application binary and its testable library
//! modules. Pure logic (theme palettes, filesystem ordering) lives in library
//! modules so it can be unit-tested without a GPU or a window, which keeps the
//! coverage bar in `docs/STANDARDS.md` reachable. The GPU and windowing glue is
//! deliberately thin.

pub mod app;
pub mod cli;
pub mod curate;
pub mod decode;
pub mod edit;
pub mod error;
pub mod fs;
pub mod gpu;
pub(crate) mod sandbox;
pub mod theme;
pub mod thumbs;
/// The main user interface module utilizing egui for overlays.
pub mod ui;
pub mod view;
pub(crate) mod worker_limit;

pub use app::{run, run_with_image};
pub use error::Error;
