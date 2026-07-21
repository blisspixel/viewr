//! viewr: a photo viewer that just shows your photos.
//!
//! This crate hosts both the application binary and its testable library
//! modules. Pure logic (theme palettes, filesystem ordering) lives in library
//! modules so it can be unit-tested without a GPU or a window, which keeps the
//! coverage bar in `docs/STANDARDS.md` reachable. The GPU and windowing glue is
//! deliberately thin.

pub mod app;
pub mod decode;
pub mod edit;
pub mod error;
pub mod fs;
pub mod gpu;
pub(crate) mod sandbox;
pub mod theme;
/// The main user interface module utilizing egui for overlays.
pub mod ui;
pub mod view;

pub use app::run;
pub use error::Error;
