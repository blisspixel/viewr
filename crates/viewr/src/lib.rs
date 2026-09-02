//! viewr: a photo viewer that just shows your photos.
//!
//! This crate hosts both the application binary and its testable library
//! modules. Pure logic (theme palettes, filesystem ordering) lives in library
//! modules so it can be unit-tested without a GPU or a window, which keeps the
//! coverage bar in `docs/STANDARDS.md` reachable. The GPU and windowing glue is
//! deliberately thin.

pub(crate) mod animated;
pub mod app;
mod chrome;
pub mod cli;
pub mod color;
pub mod crop;
mod crop_state;
pub mod curate;
mod curation_state;
mod current_work;
pub mod decode;
mod display_output;
mod display_probe;
mod display_state;
pub mod edit;
mod edit_state;
mod entry_state;
pub mod ephemeral;
pub mod error;
mod file_coherence;
mod folder_sort_preference;
pub mod fs;
pub mod gpu;
mod gpu_image;
mod gpu_policy;
pub mod heal;
pub mod image_info;
pub(crate) mod job;
mod keyboard_route;
mod locale;
#[cfg(target_os = "macos")]
mod macos;
mod mosaic;
mod open_with;
pub(crate) mod pages;
pub mod performance;
/// Folder and navigation state.
pub mod playlist;
pub mod prefetch;
mod presentation;
pub mod privacy;
mod rating_state;
/// Embedded image ratings and session-only rating filters.
pub mod ratings;
pub(crate) mod sandbox;
mod save_state;
/// State for the selected, loading, and presented image.
pub mod session;
mod shortcuts;
mod startup;
pub mod theme;
pub mod thumbs;
/// The main user interface module built with egui.
pub mod ui;
pub mod view;
mod work_currency;
pub(crate) mod worker_limit;

pub use app::{run, run_with_image};
pub use error::Error;
