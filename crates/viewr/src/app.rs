//! The application: a message loop of our own on winit's event loop. For Phase 0
//! it opens a window, sets up the GPU renderer, and clears each frame to the
//! theme background. The Elm-style shape (one state, messages, update, render)
//! is borrowed without depending on a UI framework.
#![allow(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::curate::{FlagSet, TrashedFile};
use crate::decode::DecodedImage;
use crate::error::Error;
use crate::gpu::{FrameResult, Renderer};
use crate::prefetch::{self, PrefetchCache};
use crate::theme::Mode;
use crate::thumbs::{self, ThumbRgba};
use crate::ui::FilmstripItem;

/// Start viewr: create the event loop and run the application to completion.
///
/// # Errors
/// Returns [`Error`] if the event loop cannot be created or fails while running.
pub fn run() -> Result<(), Error> {
    run_with_image(std::env::args_os().nth(1).map(PathBuf::from))
}

/// Start the GUI with an optional initial image path (from the CLI).
///
/// # Errors
/// Returns [`Error`] if the event loop cannot be created or fails while running.
pub fn run_with_image(image_path: Option<PathBuf>) -> Result<(), Error> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    // A viewer is idle most of the time; wait for events rather than spin.
    event_loop.set_control_flow(ControlFlow::Wait);
    #[cfg(target_os = "macos")]
    crate::macos::install_open_file_handler(event_loop.create_proxy())
        .map_err(|message| Error::Platform(message.to_owned()))?;
    let (thumb_result_tx, thumb_rx) = mpsc::channel();
    let (prefetch_result_tx, prefetch_rx) = mpsc::channel();
    let image_path = image_path.map(|path| crate::fs::canonical_file_path(&path).unwrap_or(path));
    let mut app = App {
        image_path,
        renderer: None,
        playlist: None,
        scanner_rx: None,
        transform: Transform::default(),
        is_fullscreen: false,
        last_trashed: Vec::new(),
        current_image: None,
        loaded_image_path: None,
        show_exif: false,
        // Privacy default: Save As strips EXIF/GPS unless the user opts in.
        retain_exif: false,
        bg_override: None,
        image_loader_rx: None,
        image_load_generation: Arc::new(AtomicU64::new(0)),
        resize_on_load: None,
        flags: FlagSet::new(),
        modifiers: ModifiersState::default(),
        toast: None,
        toast_until: None,
        last_activity: Instant::now(),
        cursor_pos: (0.0, 0.0),
        last_click: None,
        space_held: false,
        space_dragged: false,
        mouse_left_down: false,
        thumb_result_tx,
        thumb_rx,
        thumbs_in_flight: HashSet::new(),
        thumb_textures: HashMap::new(),
        prefetch: PrefetchCache::with_capacity(prefetch::DEFAULT_CAPACITY),
        prefetch_in_flight: HashSet::new(),
        prefetch_result_tx,
        prefetch_rx,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct Playlist {
    files: Vec<PathBuf>,
    index: usize,
}

enum ScanPurpose {
    SelectedFile(PathBuf),
    OpenFolder,
}

/// Application-level events delivered from native platform integrations.
pub(crate) enum UserEvent {
    /// Finder or Launch Services requested that viewr open this file.
    #[cfg_attr(
        not(target_os = "macos"),
        allow(
            dead_code,
            reason = "constructed only by the macOS Launch Services bridge"
        )
    )]
    OpenFile(PathBuf),
}

struct FolderScan {
    purpose: ScanPurpose,
    files: std::io::Result<Vec<PathBuf>>,
}

/// The aspect ratio to enforce when cropping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CropRatio {
    /// Free-form cropping with no locked ratio.
    Free,
    /// 1:1 square crop.
    Square,
    /// 4:3 crop.
    FourThree,
    /// 16:9 crop.
    SixteenNine,
}

#[derive(Clone, Copy)]
#[allow(clippy::struct_excessive_bools)] // independent mode flags for view/crop tools
struct Transform {
    zoom: f32,
    offset_x: f32,
    offset_y: f32,
    rotation_steps: i32,
    flip_h: bool,
    flip_v: bool,
    is_panning: bool,
    last_cursor: Option<(f64, f64)>,
    crop_rect: Option<[f32; 4]>,
    is_cropping: bool,
    crop_ratio: CropRatio,
    crop_start: Option<(f32, f32)>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
            rotation_steps: 0,
            flip_h: false,
            flip_v: false,
            is_panning: false,
            last_cursor: None,
            crop_rect: None,
            is_cropping: false,
            crop_ratio: CropRatio::Free,
            crop_start: None,
        }
    }
}

/// The whole application state. Deliberately small.
#[allow(clippy::struct_excessive_bools)] // independent UI/session mode bits
struct App {
    renderer: Option<Renderer>,
    image_path: Option<PathBuf>,
    playlist: Option<Playlist>,
    scanner_rx: Option<Receiver<FolderScan>>,
    transform: Transform,
    is_fullscreen: bool,
    last_trashed: Vec<TrashedFile>,
    current_image: Option<DecodedImage>,
    /// Source path corresponding exactly to `current_image` and the GPU texture.
    loaded_image_path: Option<PathBuf>,
    show_exif: bool,
    /// When true, Save As copies EXIF from the source. Default **false** (strip).
    retain_exif: bool,
    bg_override: Option<[f64; 4]>,
    image_loader_rx: Option<
        std::sync::mpsc::Receiver<(
            std::path::PathBuf,
            Result<crate::decode::DecodedImage, String>,
        )>,
    >,
    /// Monotonic cancellation token for superseded foreground decode jobs.
    image_load_generation: Arc<AtomicU64>,
    /// The explicitly opened image whose first completed load should size the window.
    resize_on_load: Option<PathBuf>,
    /// Paths flagged for batch cull (photographer workflow).
    flags: FlagSet,
    /// Latest keyboard modifiers (for Shift+Delete, etc.).
    modifiers: ModifiersState,
    /// Bottom toast message.
    toast: Option<String>,
    /// When the toast should disappear.
    toast_until: Option<Instant>,
    /// Last mouse/keyboard activity for chrome auto-hide.
    last_activity: Instant,
    /// Latest cursor position in physical pixels.
    cursor_pos: (f64, f64),
    /// Last click time/pos for double-click fit toggle.
    last_click: Option<(Instant, (f64, f64))>,
    /// Spacebar currently held (temporary hand / pan tool).
    space_held: bool,
    /// Whether a pan occurred while Space was held (skip reset on release).
    space_dragged: bool,
    /// Left mouse button currently down.
    mouse_left_down: bool,
    /// Completed thumbnail results (or failures) from background jobs.
    thumb_result_tx: Sender<Result<ThumbRgba, (PathBuf, String)>>,
    /// Receiver for thumbnail results.
    thumb_rx: Receiver<Result<ThumbRgba, (PathBuf, String)>>,
    /// Paths currently decoding for the filmstrip.
    thumbs_in_flight: HashSet<PathBuf>,
    /// Uploaded egui textures for filmstrip cells.
    thumb_textures: HashMap<PathBuf, egui::TextureHandle>,
    /// In-memory neighbor full-decode cache (never written to disk).
    prefetch: PrefetchCache,
    /// Paths currently being prefetched in the background.
    prefetch_in_flight: HashSet<PathBuf>,
    /// Sender shared by bounded speculative decode jobs.
    prefetch_result_tx: Sender<(PathBuf, Result<DecodedImage, String>)>,
    /// Completed prefetch jobs: `(path, result)`.
    prefetch_rx: Receiver<(PathBuf, Result<DecodedImage, String>)>,
}

impl App {
    fn open_file_request(&mut self, path: PathBuf) {
        self.touch_activity();
        if self.renderer.is_some() {
            self.load_and_scan(path);
        } else {
            self.image_path = Some(path);
        }
    }

    fn load_and_scan(&mut self, path: PathBuf) {
        let path = crate::fs::canonical_file_path(&path).unwrap_or(path);
        self.playlist = None;
        self.begin_image_load(path.clone(), true);
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        self.start_folder_scan(directory, ScanPurpose::SelectedFile(path));
    }

    fn begin_image_load(&mut self, path: PathBuf, resize_to_image: bool) {
        self.image_path = Some(path.clone());
        self.transform = Transform::default();
        self.resize_on_load = resize_to_image.then_some(path.clone());
        self.spawn_image_load(path);
    }

    fn open_image_dialog(&mut self) {
        let extensions = crate::fs::supported_extensions().collect::<Vec<_>>();
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &extensions)
            .pick_file()
        {
            self.load_and_scan(path);
        }
    }

    fn open_folder_dialog(&mut self) {
        if let Some(directory) = rfd::FileDialog::new().pick_folder() {
            self.start_folder_scan(directory, ScanPurpose::OpenFolder);
        }
    }

    fn start_folder_scan(&mut self, directory: PathBuf, purpose: ScanPurpose) {
        self.scanner_rx = None;
        let (sender, receiver) = mpsc::channel();
        let window = self
            .renderer
            .as_ref()
            .map(|renderer| renderer.window.clone());
        let spawn_result = std::thread::Builder::new()
            .name("viewr-folder-scan".into())
            .spawn(move || {
                let files = crate::fs::scan_images(&directory);
                let _ = sender.send(FolderScan { purpose, files });
                if let Some(window) = window {
                    window.request_redraw();
                }
            });
        match spawn_result {
            Ok(_) => self.scanner_rx = Some(receiver),
            Err(error) => {
                log::error!("failed to start folder scan");
                self.show_toast(format!("Could not scan folder: {error}"));
            }
        }
    }

    fn replace_playlist(&mut self, files: Vec<PathBuf>, index: usize) {
        self.prefetch.clear();
        self.prefetch_in_flight.clear();
        self.playlist = Some(Playlist { files, index });
    }

    fn finish_folder_scan(&mut self, scan: FolderScan) {
        if let ScanPurpose::SelectedFile(selected) = &scan.purpose
            && !selected_scan_is_current(self.image_path.as_deref(), selected)
        {
            return;
        }
        match (scan.purpose, scan.files) {
            (ScanPurpose::SelectedFile(selected), Ok(files)) => {
                if let Some(index) = selected_file_index(&files, &selected) {
                    self.replace_playlist(files, index);
                } else {
                    self.replace_playlist(vec![selected], 0);
                }
                self.kick_prefetch();
            }
            (ScanPurpose::SelectedFile(selected), Err(error)) => {
                log::warn!("folder scan unavailable: {error}");
                self.replace_playlist(vec![selected], 0);
                self.show_toast("Open Folder to allow next and previous image access");
            }
            (ScanPurpose::OpenFolder, Ok(files)) if files.is_empty() => {
                self.show_toast("The selected folder contains no supported images");
            }
            (ScanPurpose::OpenFolder, Ok(files)) => {
                let first = files[0].clone();
                self.replace_playlist(files, 0);
                self.begin_image_load(first, true);
                self.kick_prefetch();
            }
            (ScanPurpose::OpenFolder, Err(error)) => {
                log::warn!("selected folder scan failed: {error}");
                self.show_toast("Could not read the selected folder");
            }
        }
    }

    fn display_loaded_image(&mut self, path: &Path, image: DecodedImage) {
        let should_resize = self.resize_on_load.as_deref() == Some(path);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_image(&image);
            if should_resize {
                resize_window_to_image(renderer);
            }
        }
        if should_resize {
            self.resize_on_load = None;
        }
        self.current_image = Some(image);
        self.loaded_image_path = Some(path.to_owned());
    }

    fn invalidate_displayed_image(&mut self) {
        self.current_image = None;
        self.loaded_image_path = None;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_image();
        }
    }

    fn cancel_pending_image_load(&mut self) {
        self.image_load_generation.fetch_add(1, Ordering::AcqRel);
        self.image_loader_rx = None;
        self.resize_on_load = None;
    }

    fn current_loaded_path(&self) -> Option<&Path> {
        let path = self.image_path.as_deref()?;
        (self.current_image.is_some() && self.loaded_image_path.as_deref() == Some(path))
            .then_some(path)
    }

    fn screen_to_uv(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let renderer = self.renderer.as_ref()?;
        let win_size = renderer.window().inner_size();
        if win_size.width == 0 || win_size.height == 0 {
            return None;
        }
        let image_size = renderer.image_size()?;

        let ndc_x = (x as f32 / win_size.width as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - (y as f32 / win_size.height as f32) * 2.0;

        let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
        let mut p =
            crate::view::fit_to_window((win_size.width, win_size.height), image_size, rotated90);

        p.scale[0] *= self.transform.zoom;
        p.scale[1] *= self.transform.zoom;
        p.offset[0] = self.transform.offset_x;
        p.offset[1] = self.transform.offset_y;

        let corner_x = (ndc_x - p.offset[0]) / p.scale[0];
        let corner_y = (ndc_y - p.offset[1]) / p.scale[1];

        let base_uv_x = (corner_x + 1.0) * 0.5;
        let base_uv_y = (1.0 - corner_y) * 0.5;

        let cx = base_uv_x - 0.5;
        let cy = base_uv_y - 0.5;

        let rot = self.transform.rotation_steps.rem_euclid(4);
        let mut uv_matrix = match rot {
            1 => [0.0, -1.0, 1.0, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0],
            3 => [0.0, 1.0, -1.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0],
        };

        if self.transform.flip_h {
            uv_matrix[0] = -uv_matrix[0];
            uv_matrix[2] = -uv_matrix[2];
        }
        if self.transform.flip_v {
            uv_matrix[1] = -uv_matrix[1];
            uv_matrix[3] = -uv_matrix[3];
        }

        let uv_x = uv_matrix[0] * cx + uv_matrix[2] * cy + 0.5;
        let uv_y = uv_matrix[1] * cx + uv_matrix[3] * cy + 0.5;

        Some((uv_x, uv_y))
    }

    fn navigate(&mut self, delta: isize) {
        if let Some(playlist) = &self.playlist {
            if playlist.files.is_empty() {
                return;
            }
            let max_idx = playlist.files.len().saturating_sub(1).cast_signed();
            let new_index = (playlist.index.cast_signed() + delta)
                .clamp(0, max_idx)
                .cast_unsigned();
            if new_index != playlist.index {
                self.go_to_index(new_index);
            }
        }
    }

    fn go_to_index(&mut self, new_index: usize) {
        let Some(playlist) = &mut self.playlist else {
            return;
        };
        if playlist.files.is_empty() || new_index >= playlist.files.len() {
            return;
        }
        playlist.index = new_index;
        let next_path = playlist.files[new_index].clone();
        self.image_path = Some(next_path.clone());
        self.transform = Transform::default();
        self.resize_on_load = None;

        self.spawn_image_load(next_path);
        self.kick_prefetch();
    }

    /// Decode nearby playlist entries into the in-memory cache (no disk writes).
    fn kick_prefetch(&mut self) {
        let Some(playlist) = &self.playlist else {
            return;
        };
        let targets: Vec<PathBuf> =
            prefetch::neighbor_indices(playlist.index, playlist.files.len(), 2)
                .into_iter()
                .map(|i| playlist.files[i].clone())
                .filter(|p| !self.prefetch.contains(p) && !self.prefetch_in_flight.contains(p))
                .collect();
        if targets.is_empty() {
            return;
        }

        for path in targets {
            let job_path = path.clone();
            let tx = self.prefetch_result_tx.clone();
            let window = self.renderer.as_ref().map(|r| r.window.clone());
            let scheduled = crate::decode::schedule_background_decode(move || {
                let res = DecodedImage::load_background(&job_path).map_err(|e| e.to_string());
                let _ = tx.send((job_path, res));
                if let Some(w) = window {
                    w.request_redraw();
                }
            });
            if scheduled {
                self.prefetch_in_flight.insert(path);
            }
        }
    }

    fn poll_prefetch(&mut self) {
        let mut completed = false;
        while let Ok((path, result)) = self.prefetch_rx.try_recv() {
            completed = true;
            self.prefetch_in_flight.remove(&path);
            if let Ok(image) = result {
                // Do not cache the currently displayed path as a neighbor entry;
                // it already lives in `current_image`. Also discard results from
                // a folder that was replaced while this decode was queued.
                if self.image_path.as_ref() != Some(&path)
                    && self
                        .playlist
                        .as_ref()
                        .is_some_and(|playlist| playlist.files.contains(&path))
                {
                    self.prefetch.insert(path, image);
                }
            }
        }
        if completed {
            self.kick_prefetch();
        }
    }

    fn toggle_flag_current(&mut self) {
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        let flagged = self.flags.toggle(&path);
        // Session-only memory; never write flag state to disk.
        self.show_toast(if flagged {
            format!("Flagged · {} total", self.flags.len())
        } else {
            format!("Unflagged · {} remaining", self.flags.len())
        });
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }

    fn trash_current(&mut self) {
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };

        let receipt = match crate::curate::move_to_trash(&path) {
            Ok(receipt) => receipt,
            Err(e) => {
                log::error!("failed to move file to trash");
                self.show_toast(format!("Trash failed: {e}"));
                return;
            }
        };

        let playlist_index = self.playlist.as_ref().map_or(0, |p| p.index);
        self.last_trashed = vec![TrashedFile {
            receipt,
            playlist_index,
        }];
        self.flags.remove(&path);
        self.show_toast("Moved to trash · Undo with U");
        self.after_paths_removed(&[path], playlist_index);
    }

    fn trash_flagged(&mut self) {
        let flagged = self.flags.take_all_sorted();
        if flagged.is_empty() {
            return;
        }
        let current_index = self.playlist.as_ref().map_or(0, |p| p.index);
        let playlist_indices = self
            .playlist
            .as_ref()
            .map_or_else(HashMap::new, |playlist| {
                playlist
                    .files
                    .iter()
                    .enumerate()
                    .map(|(index, path)| (path.clone(), index))
                    .collect()
            });
        let (ok, failed) = crate::curate::trash_many(&flagged);
        if !failed.is_empty() {
            log::error!("batch trash partial failure");
            for (path, _) in &failed {
                self.flags.insert(path.clone());
            }
        }
        if !ok.is_empty() {
            self.last_trashed = ok
                .into_iter()
                .map(|receipt| {
                    let playlist_index = playlist_indices
                        .get(receipt.original_path())
                        .copied()
                        .unwrap_or(current_index);
                    TrashedFile {
                        receipt,
                        playlist_index,
                    }
                })
                .collect();
            let removed = self
                .last_trashed
                .iter()
                .map(|record| record.receipt.original_path().to_owned())
                .collect::<Vec<_>>();
            if failed.is_empty() {
                self.show_toast(format!("Trashed {} file(s) · Undo with U", removed.len()));
            } else {
                self.show_toast(format!(
                    "Trashed {}; {} failed · Undo with U",
                    removed.len(),
                    failed.len()
                ));
            }
            self.after_paths_removed(&removed, current_index);
        } else if let Some((_, error)) = failed.first() {
            self.show_toast(format!("Trash failed: {error}"));
        }
    }

    fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(msg.into());
        self.toast_until = Some(Instant::now() + Duration::from_secs(3));
        if let Some(r) = self.renderer.as_ref() {
            r.window().request_redraw();
        }
    }

    fn cancel_crop(&mut self) {
        self.transform.is_cropping = false;
        self.transform.crop_rect = None;
        self.transform.crop_start = None;
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }

    fn touch_activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Project image UV to screen pixels (inverse of [`Self::screen_to_uv`]).
    fn uv_to_screen(&self, uv_x: f32, uv_y: f32) -> Option<(f32, f32)> {
        let renderer = self.renderer.as_ref()?;
        let win_size = renderer.window().inner_size();
        if win_size.width == 0 || win_size.height == 0 {
            return None;
        }
        let image_size = renderer.image_size()?;
        let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
        let mut place =
            crate::view::fit_to_window((win_size.width, win_size.height), image_size, rotated90);
        place.scale[0] *= self.transform.zoom;
        place.scale[1] *= self.transform.zoom;
        place.offset[0] = self.transform.offset_x;
        place.offset[1] = self.transform.offset_y;

        let rot = self.transform.rotation_steps.rem_euclid(4);
        let mut matrix = match rot {
            1 => [0.0_f32, -1.0, 1.0, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0],
            3 => [0.0, 1.0, -1.0, 0.0],
            _ => [1.0, 0.0, 0.0, 1.0],
        };
        if self.transform.flip_h {
            matrix[0] = -matrix[0];
            matrix[2] = -matrix[2];
        }
        if self.transform.flip_v {
            matrix[1] = -matrix[1];
            matrix[3] = -matrix[3];
        }
        // uv = M * centered + 0.5; invert M.
        let det = matrix[0] * matrix[3] - matrix[1] * matrix[2];
        if det.abs() < 1e-8 {
            return None;
        }
        let du = uv_x - 0.5;
        let dv = uv_y - 0.5;
        let centered_x = (matrix[3] * du - matrix[2] * dv) / det;
        let centered_y = (-matrix[1] * du + matrix[0] * dv) / det;
        let base_u = centered_x + 0.5;
        let base_v = centered_y + 0.5;
        let corner_x = base_u * 2.0 - 1.0;
        let corner_y = 1.0 - base_v * 2.0;
        let ndc_x = corner_x * place.scale[0] + place.offset[0];
        let ndc_y = corner_y * place.scale[1] + place.offset[1];
        let screen_x = (ndc_x + 1.0) * 0.5 * win_size.width as f32;
        let screen_y = (1.0 - ndc_y) * 0.5 * win_size.height as f32;
        Some((screen_x, screen_y))
    }

    fn crop_screen_rect(&self) -> Option<[f32; 4]> {
        let rect = self.transform.crop_rect?;
        let (x0, y0) = self.uv_to_screen(rect[0], rect[1])?;
        let (x1, y1) = self.uv_to_screen(rect[2], rect[3])?;
        Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
    }

    fn zoom_at_cursor(&mut self, factor: f32) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let win = renderer.window().inner_size();
        let Some(ndc) = crate::view::cursor_to_ndc(self.cursor_pos, (win.width, win.height)) else {
            return;
        };
        let old = self.transform.zoom;
        let new_zoom = (old * factor).clamp(0.05, 64.0);
        let applied = new_zoom / old;
        if (applied - 1.0).abs() < 1e-6 {
            return;
        }
        let off = crate::view::pan_after_zoom_at_cursor(
            [self.transform.offset_x, self.transform.offset_y],
            ndc,
            applied,
        );
        self.transform.zoom = new_zoom;
        self.transform.offset_x = off[0];
        self.transform.offset_y = off[1];
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }

    fn navigate_to(&mut self, index: usize) {
        let Some(playlist) = &self.playlist else {
            return;
        };
        if playlist.files.is_empty() || index >= playlist.files.len() {
            return;
        }
        if index == playlist.index {
            return;
        }
        self.go_to_index(index);
    }

    /// Window of playlist entries around the current index for the filmstrip.
    fn filmstrip_entries(&self) -> Vec<FilmstripItem> {
        let Some(playlist) = &self.playlist else {
            return Vec::new();
        };
        if playlist.files.is_empty() {
            return Vec::new();
        }
        let n = playlist.files.len();
        let start = playlist.index.saturating_sub(4);
        let end = (playlist.index + 5).min(n);
        (start..end)
            .map(|i| {
                let path = &playlist.files[i];
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |s| s.to_string_lossy().into_owned(),
                );
                let flagged = self.flags.contains(path);
                let texture = self.thumb_textures.get(path).cloned();
                FilmstripItem {
                    index: i,
                    name,
                    flagged,
                    texture,
                }
            })
            .collect()
    }

    fn request_thumbs_for_filmstrip(&mut self) {
        let Some(playlist) = &self.playlist else {
            return;
        };
        if playlist.files.is_empty() {
            return;
        }
        let n = playlist.files.len();
        let start = playlist.index.saturating_sub(4);
        let end = (playlist.index + 5).min(n);
        let paths: Vec<PathBuf> = playlist.files[start..end].to_vec();
        for path in paths {
            if self.thumb_textures.contains_key(&path) || self.thumbs_in_flight.contains(&path) {
                continue;
            }
            let job_path = path.clone();
            let tx = self.thumb_result_tx.clone();
            let scheduled = crate::decode::schedule_background_decode(move || {
                let result = match thumbs::generate_thumb(&job_path) {
                    Ok(thumb) => Ok(thumb),
                    Err(err) => Err((job_path, err)),
                };
                let _ = tx.send(result);
            });
            if scheduled {
                self.thumbs_in_flight.insert(path);
            }
        }
    }

    fn poll_thumbnails(&mut self) {
        let mut got = false;
        while let Ok(msg) = self.thumb_rx.try_recv() {
            got = true;
            match msg {
                Ok(thumb) => {
                    self.thumbs_in_flight.remove(&thumb.path);
                    if let Some(renderer) = &self.renderer {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [thumb.width as usize, thumb.height as usize],
                            &thumb.rgba,
                        );
                        let id = format!("thumb:{}", thumb.path.display());
                        let handle =
                            renderer
                                .egui_ctx
                                .load_texture(id, image, egui::TextureOptions::LINEAR);
                        self.thumb_textures.insert(thumb.path, handle);
                    }
                }
                Err((path, err)) => {
                    self.thumbs_in_flight.remove(&path);
                    // Filename-only (no directory) if the user opted into RUST_LOG.
                    log::debug!("thumb failed: {err}");
                }
            }
        }
        if got && let Some(r) = self.renderer.as_ref() {
            r.window().request_redraw();
        }
        if got {
            self.kick_prefetch();
        }
    }

    fn toggle_fit_actual(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let win = renderer.window().inner_size();
        let Some(image) = renderer.image_size() else {
            return;
        };
        let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
        let (iw, ih) = if rotated90 {
            (image.1 as f32, image.0 as f32)
        } else {
            (image.0 as f32, image.1 as f32)
        };
        let (vw, vh) = (win.width as f32, win.height as f32);
        if iw <= 0.0 || ih <= 0.0 || vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let fit_s = (vw / iw).min(vh / ih);
        let actual_zoom = if fit_s > 0.0 { 1.0 / fit_s } else { 1.0 };
        if (self.transform.zoom - 1.0).abs() < 0.05 {
            self.transform.zoom = actual_zoom;
            self.transform.offset_x = 0.0;
            self.transform.offset_y = 0.0;
        } else {
            self.transform.zoom = 1.0;
            self.transform.offset_x = 0.0;
            self.transform.offset_y = 0.0;
        }
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }

    fn permanent_delete_current(&mut self) {
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |s| s.to_string_lossy().into_owned(),
        );
        let confirmed = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Permanently delete?")
            .set_description(format!(
                "Delete \"{name}\" forever?\n\nThis skips the Recycle Bin and cannot be undone from viewr."
            ))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show();
        if confirmed != rfd::MessageDialogResult::Ok {
            return;
        }
        if let Err(e) = crate::curate::permanent_delete(&path) {
            log::error!("permanent delete failed");
            self.show_toast(format!("Delete failed: {e}"));
            return;
        }
        self.flags.remove(&path);
        let playlist_index = self.playlist.as_ref().map_or(0, |p| p.index);
        self.last_trashed.clear(); // not restorable
        self.after_paths_removed(&[path], playlist_index);
    }

    fn after_paths_removed(&mut self, removed: &[PathBuf], old_index: usize) {
        if let Some(playlist) = &mut self.playlist {
            crate::curate::remove_from_playlist(&mut playlist.files, removed);
            if playlist.files.is_empty() {
                self.cancel_pending_image_load();
                self.image_path = None;
                self.invalidate_displayed_image();
            } else {
                playlist.index =
                    crate::curate::index_after_removals(&playlist.files, old_index, removed);
                let next_path = playlist.files[playlist.index].clone();
                self.image_path = Some(next_path.clone());
                self.transform = Transform::default();
                self.spawn_image_load(next_path);
            }
        } else {
            self.cancel_pending_image_load();
            self.image_path = None;
            self.invalidate_displayed_image();
        }
    }

    fn undo_trash(&mut self) {
        if self.last_trashed.is_empty() {
            return;
        }

        let records = std::mem::take(&mut self.last_trashed);
        let mut outcome = crate::curate::restore_trash_batch(records);
        outcome.restored.sort_by_key(|record| record.playlist_index);
        let restored_count = outcome.restored.len();
        let first_restored_path = outcome
            .restored
            .first()
            .map(|record| record.receipt.original_path().to_owned());
        if let Some(playlist) = &mut self.playlist {
            let mut focused_index = None;
            for record in &outcome.restored {
                let index =
                    crate::curate::restored_playlist_index(record.playlist_index, &outcome.failed)
                        .min(playlist.files.len());
                focused_index.get_or_insert(index);
                playlist
                    .files
                    .insert(index, record.receipt.original_path().to_owned());
            }
            if let Some(index) = focused_index {
                playlist.index = index.min(playlist.files.len().saturating_sub(1));
            }
        }
        self.last_trashed = outcome.failed;

        if let Some(original_path) = first_restored_path {
            self.image_path = Some(original_path.clone());
            self.transform = Transform::default();
            self.spawn_image_load(original_path);
        }

        if self.last_trashed.is_empty() {
            self.show_toast(format!("Restored {restored_count} file(s)"));
        } else if restored_count == 0 {
            log::error!("failed to restore from trash");
            self.show_toast(format!(
                "Restore failed: {}",
                outcome.first_error.as_deref().unwrap_or("unknown error")
            ));
        } else {
            log::error!("batch restore partial failure");
            self.show_toast(format!(
                "Restored {restored_count}; {} failed",
                self.last_trashed.len()
            ));
        }
    }

    fn spawn_image_load(&mut self, path: PathBuf) {
        let generation = self
            .image_load_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.invalidate_displayed_image();

        // Prefer RAM cache even for non-navigate loads (undo, filmstrip jump).
        if let Some(image) = self.prefetch.take(&path) {
            self.display_loaded_image(&path, image);
            self.image_loader_rx = None;
            self.kick_prefetch();
            if let Some(r) = self.renderer.as_ref() {
                r.window().request_redraw();
            }
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.image_loader_rx = Some(rx);
        let window = self.renderer.as_ref().map(|r| r.window.clone());
        let current_generation = Arc::clone(&self.image_load_generation);
        let scheduled = crate::decode::schedule_foreground_decode(move || {
            let res = match DecodedImage::load_if_current(&path, &current_generation, generation) {
                Ok(Some(image)) => Ok(image),
                Ok(None) => return,
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send((path, res));
            if let Some(w) = window {
                w.request_redraw();
            }
        });
        if let Err(error) = scheduled {
            self.image_loader_rx = None;
            log::error!("failed to queue foreground decode");
            self.show_toast(format!("Could not start image decode: {error}"));
        }
    }

    /// Mark the current image as a pick for this session only.
    ///
    /// Deliberately never writes `_picks.txt` or any other side-file next to
    /// the user's photos. Flag with `X` for the batch-cull workflow.
    fn star_current(&mut self) {
        // Alias to the in-memory flag set so S and X stay privacy-safe.
        self.toggle_flag_current();
    }

    fn save_as(&mut self) {
        if let Some(path) = &self.image_path
            && let Some(image) = &self.current_image
            && self.loaded_image_path.as_ref() == Some(path)
        {
            let default_name = path.with_extension("jpg");
            if let Some(file_name) = default_name.file_name()
                && let Some(save_path) = rfd::FileDialog::new()
                    .set_file_name(file_name.to_string_lossy())
                    .add_filter("JPEG", &["jpg", "jpeg"])
                    .add_filter("PNG", &["png"])
                    .add_filter("WebP", &["webp"])
                    .add_filter("BMP", &["bmp"])
                    .save_file()
            {
                let opts = if self.retain_exif {
                    crate::edit::SaveOptions::retain_exif()
                } else {
                    crate::edit::SaveOptions::strip()
                };
                match crate::edit::save_with_options(image, &save_path, Some(path), opts) {
                    Ok(()) => {
                        if self.retain_exif {
                            self.show_toast("Saved · EXIF retained");
                        } else {
                            self.show_toast("Saved · metadata stripped");
                        }
                    }
                    Err(e) => {
                        log::error!("failed to save image");
                        self.show_toast(format!("Save failed: {e}"));
                    }
                }
            }
        }
    }

    /// Convert a UV crop rect (0..1) into pixel coordinates for [`crate::edit::crop`].
    fn crop_pixel_rect(rect: [f32; 4], width: u32, height: u32) -> Option<crate::edit::Rect> {
        let cw = width as f32;
        let ch = height as f32;
        let x = nonneg_round_u32(rect[0] * cw);
        let y = nonneg_round_u32(rect[1] * ch);
        let w = nonneg_round_u32((rect[2] - rect[0]) * cw);
        let h = nonneg_round_u32((rect[3] - rect[1]) * ch);
        if w == 0 || h == 0 {
            return None;
        }
        Some(crate::edit::Rect {
            x,
            y,
            width: w,
            height: h,
        })
    }

    fn apply_crop_rect(&mut self) {
        let Some(rect) = self.transform.crop_rect else {
            return;
        };
        let Some(image) = &self.current_image else {
            return;
        };
        let Some(pixel_rect) = Self::crop_pixel_rect(rect, image.width, image.height) else {
            return;
        };
        let cropped = crate::edit::crop(image, pixel_rect);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_image(&cropped);
        }
        self.current_image = Some(cropped);
        self.transform = Transform::default();
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }
}

/// Resize the window to fit the loaded image within the current monitor.
fn resize_window_to_image(renderer: &Renderer) {
    let Some(monitor) = renderer.window().current_monitor() else {
        return;
    };
    let Some((width, height)) = renderer.image_size() else {
        return;
    };
    let monitor_scale = monitor.scale_factor();
    let available_width = f64::from(monitor.size().width) / monitor_scale;
    let available_height = f64::from(monitor.size().height) / monitor_scale;
    let image_width = f64::from(width);
    let image_height = f64::from(height);
    let fit_scale = ((available_width * 0.8) / image_width)
        .min((available_height * 0.8) / image_height)
        .min(1.0);
    let size = LogicalSize::new(image_width * fit_scale, image_height * fit_scale);
    let _ = renderer.window().request_inner_size(size);
}

/// Round a non-negative f32 to u32 without triggering sign-loss noise.
fn nonneg_round_u32(v: f32) -> u32 {
    let v = v.round().max(0.0);
    if v >= f32::from(u16::MAX) {
        u32::from(u16::MAX)
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            v as u32
        }
    }
}

fn load_icon() -> Option<winit::window::Icon> {
    let bytes = include_bytes!("../../../assets/icon.ico");
    if let Ok(image) = image::load_from_memory(bytes) {
        let rgba = image.into_rgba8();
        let (width, height) = rgba.dimensions();
        winit::window::Icon::from_rgba(rgba.into_raw(), width, height).ok()
    } else {
        None
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title("viewr")
            .with_inner_size(LogicalSize::new(1100.0, 750.0))
            .with_visible(false);

        if let Some(icon) = load_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("failed to create window: {e}");
                event_loop.exit();
                return;
            }
        };

        let mode = Mode::from_winit_or_dark(window.theme());
        match pollster::block_on(Renderer::new(window, mode)) {
            Ok(renderer) => {
                let initial_path = self.image_path.clone();
                self.renderer = Some(renderer);

                if let Some(path) = initial_path {
                    self.load_and_scan(path);
                }

                let _ = self.renderer.as_mut().unwrap().render(None, |_| {});
                self.renderer.as_ref().unwrap().window().set_visible(true);
            }
            Err(e) => {
                log::error!("failed to initialize gpu: {e}");
                event_loop.exit();
            }
        }
    }

    #[allow(clippy::too_many_lines)] // single key/mouse dispatch table; splitting scatters bindings
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.renderer.is_none() {
            return;
        }

        if let Some(renderer) = &mut self.renderer
            && renderer.window().id() == window_id
        {
            let window = renderer.window.clone();
            let response = renderer.egui_state.on_window_event(window.as_ref(), &event);
            if response.consumed {
                return;
            }
        }

        match event {
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::DroppedFile(path) => {
                self.touch_activity();
                self.load_and_scan(path);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.touch_activity();
                let steps = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => {
                        // Trackpad: ~50 px ≈ one detent.
                        ((p.y as f32) / 50.0).clamp(-4.0, 4.0)
                    }
                };
                if steps.abs() > f32::EPSILON {
                    let factor = 1.15_f32.powf(steps);
                    self.zoom_at_cursor(factor);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                self.touch_activity();
                let pressed = state == winit::event::ElementState::Pressed;
                self.mouse_left_down = pressed;
                if pressed && !self.transform.is_cropping {
                    let now = Instant::now();
                    let pos = self.cursor_pos;
                    if let Some((t, (lx, ly))) = self.last_click {
                        let near = (pos.0 - lx).hypot(pos.1 - ly) < 6.0;
                        if near && now.duration_since(t) < Duration::from_millis(350) {
                            self.toggle_fit_actual();
                            self.last_click = None;
                            return;
                        }
                    }
                    self.last_click = Some((now, pos));
                }
                if self.transform.is_cropping && !self.space_held {
                    if pressed {
                        if let Some((x, y)) = self.transform.last_cursor {
                            self.transform.crop_start = self.screen_to_uv(x, y);
                            self.transform.crop_rect = None;
                            self.renderer.as_mut().unwrap().window().request_redraw();
                        }
                    } else {
                        self.transform.crop_start = None;
                    }
                } else {
                    self.transform.is_panning = pressed;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = (position.x, position.y);
                self.touch_activity();
                if self.transform.is_cropping
                    && let Some(start) = self.transform.crop_start
                    && let Some(end) = self.screen_to_uv(position.x, position.y)
                {
                    let mut u_min = start.0.min(end.0).clamp(0.0, 1.0);
                    let mut v_min = start.1.min(end.1).clamp(0.0, 1.0);
                    let mut u_max = start.0.max(end.0).clamp(0.0, 1.0);
                    let mut v_max = start.1.max(end.1).clamp(0.0, 1.0);

                    if self.transform.crop_ratio != CropRatio::Free
                        && let Some(renderer) = self.renderer.as_ref()
                        && let Some((img_w, img_h)) = renderer.image_size()
                    {
                        // Free is excluded by the outer guard; grouped with Square for exhaustiveness.
                        let target_ratio = match self.transform.crop_ratio {
                            CropRatio::FourThree => 4.0 / 3.0,
                            CropRatio::SixteenNine => 16.0 / 9.0,
                            CropRatio::Square | CropRatio::Free => 1.0,
                        };

                        let width = (u_max - u_min) * (img_w as f32);
                        let height = (v_max - v_min) * (img_h as f32);

                        if height > 0.0 {
                            let current_ratio = width / height;
                            if current_ratio > target_ratio {
                                let new_width = height * target_ratio;
                                let u_diff = new_width / (img_w as f32);
                                if end.0 > start.0 {
                                    u_max = u_min + u_diff;
                                } else {
                                    u_min = u_max - u_diff;
                                }
                            } else {
                                let new_height = width / target_ratio;
                                let v_diff = new_height / (img_h as f32);
                                if end.1 > start.1 {
                                    v_max = v_min + v_diff;
                                } else {
                                    v_min = v_max - v_diff;
                                }
                            }
                        }
                    }
                    self.transform.crop_rect = Some([
                        u_min.clamp(0.0, 1.0),
                        v_min.clamp(0.0, 1.0),
                        u_max.clamp(0.0, 1.0),
                        v_max.clamp(0.0, 1.0),
                    ]);
                    self.renderer.as_mut().unwrap().window().request_redraw();
                } else if self.mouse_left_down
                    && (self.transform.is_panning || self.space_held)
                    && let Some((last_x, last_y)) = self.transform.last_cursor
                {
                    if self.space_held {
                        self.space_dragged = true;
                    }
                    let dx = position.x - last_x;
                    let dy = position.y - last_y;
                    let win_size = self.renderer.as_mut().unwrap().window().inner_size();

                    self.transform.offset_x += (dx as f32) / (win_size.width as f32 / 2.0);
                    self.transform.offset_y -= (dy as f32) / (win_size.height as f32 / 2.0);

                    self.renderer.as_mut().unwrap().window().request_redraw();
                }
                self.transform.last_cursor = Some((position.x, position.y));
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        state, logical_key, ..
                    },
                ..
            } => {
                use winit::keyboard::{Key, NamedKey};
                self.touch_activity();
                let pressed = state == winit::event::ElementState::Pressed;
                // Space: hold = temporary hand tool; tap (no drag) = reset view.
                let is_space = matches!(&logical_key, Key::Named(NamedKey::Space))
                    || matches!(&logical_key, Key::Character(c) if c.as_str() == " ");
                if is_space {
                    if pressed {
                        self.space_held = true;
                        self.space_dragged = false;
                    } else {
                        self.space_held = false;
                        if !self.space_dragged {
                            self.transform = Transform::default();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        self.space_dragged = false;
                    }
                    return;
                }
                if !pressed {
                    return;
                }
                match logical_key {
                    Key::Character(c)
                        if (c == "o" || c == "O")
                            && self.modifiers.control_key()
                            && self.modifiers.shift_key() =>
                    {
                        self.open_folder_dialog();
                    }
                    Key::Character(c) if (c == "o" || c == "O") && self.modifiers.control_key() => {
                        self.open_image_dialog();
                    }
                    Key::Character(c) if c == "o" || c == "O" => {
                        self.open_image_dialog();
                    }
                    Key::Character(c) if c == "r" || c == "R" => {
                        self.transform.rotation_steps += 1;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "l" || c == "L" => {
                        self.transform.rotation_steps -= 1;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "h" || c == "H" => {
                        self.transform.flip_h = !self.transform.flip_h;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "v" || c == "V" => {
                        self.transform.flip_v = !self.transform.flip_v;
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Character(c) if c == "s" || c == "S" => {
                        self.star_current();
                    }
                    Key::Character(c) if c == "w" || c == "W" => {
                        self.save_as();
                    }
                    Key::Character(c) if c == "c" || c == "C" => {
                        self.transform.is_cropping = !self.transform.is_cropping;
                        if !self.transform.is_cropping {
                            self.transform.crop_rect = None;
                            self.transform.crop_start = None;
                        }
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.apply_crop_rect();
                    }
                    Key::Named(NamedKey::Escape) => {
                        if self.transform.is_cropping {
                            self.cancel_crop();
                        }
                    }
                    Key::Character(c) if c == "u" || c == "U" => {
                        self.undo_trash();
                        self.renderer.as_mut().unwrap().window().request_redraw();
                    }
                    // Flag for batch cull (photographer workflow). F remains fullscreen.
                    Key::Character(c) if c == "x" || c == "X" => {
                        self.toggle_flag_current();
                    }
                    // Batch-trash all flagged files.
                    Key::Character(c) if c == "b" || c == "B" => {
                        self.trash_flagged();
                        if let Some(r) = self.renderer.as_mut() {
                            r.window().request_redraw();
                        }
                    }
                    Key::Character(c) if c == "f" || c == "F" => {
                        self.is_fullscreen = !self.is_fullscreen;
                        let renderer = self.renderer.as_mut().unwrap();
                        if self.is_fullscreen {
                            renderer
                                .window()
                                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                        } else {
                            renderer.window().set_fullscreen(None);
                        }
                    }
                    Key::Named(NamedKey::ArrowRight) => self.navigate(1),
                    Key::Named(NamedKey::ArrowLeft) => self.navigate(-1),
                    Key::Named(NamedKey::Home) => self.navigate(-999_999),
                    Key::Named(NamedKey::End) => self.navigate(999_999),
                    Key::Named(NamedKey::Delete | NamedKey::Backspace) => {
                        if self.modifiers.shift_key() {
                            // Only permanent delete asks for confirmation (modal).
                            self.permanent_delete_current();
                        } else {
                            self.trash_current();
                        }
                        if let Some(r) = self.renderer.as_mut() {
                            r.window().request_redraw();
                        }
                    }
                    Key::Named(NamedKey::F11) => {
                        self.is_fullscreen = !self.is_fullscreen;
                        let renderer = self.renderer.as_mut().unwrap();
                        if self.is_fullscreen {
                            renderer
                                .window()
                                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                        } else {
                            renderer.window().set_fullscreen(None);
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                let renderer = self.renderer.as_mut().unwrap();
                renderer.resize(size.width, size.height);
                renderer.window().request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => {
                let renderer = self.renderer.as_mut().unwrap();
                renderer.set_mode(Mode::from_winit(theme));
                renderer.window().request_redraw();
            }
            WindowEvent::RedrawRequested => {
                let mut ui_actions = Vec::new();
                if let Some(until) = self.toast_until
                    && Instant::now() > until
                {
                    self.toast = None;
                    self.toast_until = None;
                }
                // Snapshot UI/transform state before exclusive borrow of the renderer.
                let crop_screen = self.crop_screen_rect();
                let chrome_visible = self.transform.is_cropping
                    || self.last_activity.elapsed() < Duration::from_millis(2800);
                let mouse_near_left = self.cursor_pos.0 < 72.0;
                let win_h = self
                    .renderer
                    .as_ref()
                    .map_or(0.0, |r| f64::from(r.window().inner_size().height));
                let mouse_near_bottom = win_h > 0.0 && self.cursor_pos.1 > win_h - 80.0;
                let playlist_pos = self
                    .playlist
                    .as_ref()
                    .map(|p| (p.index.saturating_add(1), p.files.len().max(1)));
                self.request_thumbs_for_filmstrip();
                self.poll_thumbnails();
                let filmstrip = self.filmstrip_entries();
                let is_flagged = self
                    .image_path
                    .as_ref()
                    .is_some_and(|p| self.flags.contains(p));
                let flag_count = self.flags.len();
                let zoom = self.transform.zoom;
                let toast = self.toast.clone();
                let path_str = self
                    .image_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned());
                let show_exif = self.show_exif;
                let retain_exif = self.retain_exif;
                let is_cropping = self.transform.is_cropping;
                let crop_ratio = self.transform.crop_ratio;
                let is_panning =
                    self.transform.is_panning || (self.space_held && self.mouse_left_down);
                let bg_override = self.bg_override;
                let zoom_t = self.transform.zoom;
                let offset_x = self.transform.offset_x;
                let offset_y = self.transform.offset_y;
                let rot_steps = self.transform.rotation_steps;
                let flip_h = self.transform.flip_h;
                let flip_v = self.transform.flip_v;
                let crop_rect = self.transform.crop_rect;

                let renderer = self.renderer.as_mut().unwrap();
                renderer.window().set_visible(true);

                if let Some(bg) = bg_override {
                    renderer.set_clear_color(bg);
                } else {
                    renderer.set_mode(crate::theme::Mode::from_winit_or_dark(
                        renderer.window().theme(),
                    ));
                }

                let placement = if let Some(size) = renderer.image_size() {
                    let win_size = renderer.window().inner_size();
                    let rotated90 = rot_steps.rem_euclid(2) != 0;
                    let mut p = crate::view::fit_to_window(
                        (win_size.width, win_size.height),
                        size,
                        rotated90,
                    );

                    p.scale[0] *= zoom_t;
                    p.scale[1] *= zoom_t;
                    p.offset[0] = offset_x;
                    p.offset[1] = offset_y;

                    let rot = rot_steps.rem_euclid(4);
                    let mut uv_matrix = match rot {
                        1 => [0.0, -1.0, 1.0, 0.0],
                        2 => [-1.0, 0.0, 0.0, -1.0],
                        3 => [0.0, 1.0, -1.0, 0.0],
                        _ => [1.0, 0.0, 0.0, 1.0],
                    };

                    if flip_h {
                        uv_matrix[0] = -uv_matrix[0];
                        uv_matrix[2] = -uv_matrix[2];
                    }
                    if flip_v {
                        uv_matrix[1] = -uv_matrix[1];
                        uv_matrix[3] = -uv_matrix[3];
                    }
                    p.uv_matrix = uv_matrix;

                    if let Some(cr) = crop_rect {
                        p.crop_rect = cr;
                    }

                    Some(p)
                } else {
                    None
                };

                let img_size = renderer.image_size();
                let frame = crate::ui::UiFrameOwned {
                    show_exif,
                    retain_exif,
                    file_path: path_str,
                    img_size,
                    is_cropping,
                    crop_ratio,
                    is_panning,
                    is_flagged,
                    flag_count,
                    has_image: img_size.is_some(),
                    playlist_pos,
                    zoom,
                    toast,
                    chrome_visible: chrome_visible || mouse_near_left || mouse_near_bottom,
                    mouse_near_left,
                    mouse_near_bottom,
                    filmstrip,
                    crop_screen,
                };

                match renderer.render(placement, |ui| {
                    ui_actions = crate::ui::render(ui, &frame);
                }) {
                    FrameResult::Presented | FrameResult::Skipped => {}
                    FrameResult::NeedsReconfigure => renderer.reconfigure(),
                }

                // Keep redrawing while toast/chrome animations are live.
                if self.toast.is_some() || chrome_visible {
                    renderer.window().request_redraw();
                }

                for action in ui_actions {
                    match action {
                        crate::ui::UiAction::Open => {
                            self.open_image_dialog();
                        }
                        crate::ui::UiAction::OpenFolder => self.open_folder_dialog(),
                        crate::ui::UiAction::SaveAs => self.save_as(),
                        crate::ui::UiAction::Trash => {
                            self.trash_current();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleFlag => self.toggle_flag_current(),
                        crate::ui::UiAction::TrashFlagged => {
                            self.trash_flagged();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::PermanentDelete => {
                            self.permanent_delete_current();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::UndoTrash => {
                            self.undo_trash();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::SetBackground(bg) => {
                            self.bg_override = bg;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleExif => {
                            self.show_exif = !self.show_exif;
                        }
                        crate::ui::UiAction::ToggleRetainExif => {
                            self.retain_exif = !self.retain_exif;
                            self.show_toast(if self.retain_exif {
                                "Save As will retain EXIF (session only)"
                            } else {
                                "Save As will strip metadata (default)"
                            });
                        }
                        crate::ui::UiAction::RotateCw => {
                            self.transform.rotation_steps += 1;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::RotateCcw => {
                            self.transform.rotation_steps -= 1;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::FlipH => {
                            self.transform.flip_h = !self.transform.flip_h;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::FlipV => {
                            self.transform.flip_v = !self.transform.flip_v;
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleFullscreen => {
                            self.is_fullscreen = !self.is_fullscreen;
                            let renderer = self.renderer.as_mut().unwrap();
                            if self.is_fullscreen {
                                renderer.window().set_fullscreen(Some(
                                    winit::window::Fullscreen::Borderless(None),
                                ));
                            } else {
                                renderer.window().set_fullscreen(None);
                            }
                        }
                        crate::ui::UiAction::Navigate(d) => self.navigate(d),
                        crate::ui::UiAction::NavigateTo(i) => self.navigate_to(i),
                        crate::ui::UiAction::ToggleCrop => {
                            self.transform.is_cropping = !self.transform.is_cropping;
                            if !self.transform.is_cropping {
                                self.transform.crop_rect = None;
                                self.transform.crop_start = None;
                            }
                            self.touch_activity();
                            if let Some(r) = self.renderer.as_mut() {
                                r.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ApplyCrop => {
                            self.apply_crop_rect();
                        }
                        crate::ui::UiAction::CancelCrop => {
                            self.cancel_crop();
                        }
                        crate::ui::UiAction::SetCropRatio(r) => {
                            self.transform.crop_ratio = r;
                            if let Some(renderer) = self.renderer.as_mut() {
                                renderer.window().request_redraw();
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::OpenFile(path) => self.open_file_request(path),
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.poll_thumbnails();
        self.poll_prefetch();
        if let Some(rx) = &self.image_loader_rx
            && let Ok((path, result)) = rx.try_recv()
        {
            self.image_loader_rx = None;
            let is_current = self.image_path.as_ref() == Some(&path);
            match result {
                Ok(image) if is_current => {
                    self.display_loaded_image(&path, image);
                    self.kick_prefetch();
                    if let Some(r) = self.renderer.as_mut() {
                        r.window().request_redraw();
                    }
                }
                Ok(image) => {
                    // Late load that is no longer current still seeds the RAM cache.
                    self.prefetch.insert(path, image);
                }
                Err(e) if is_current => {
                    if self.resize_on_load.as_ref() == Some(&path) {
                        self.resize_on_load = None;
                    }
                    log::error!("decode failed");
                    self.show_toast(format!("Could not decode: {e}"));
                    if let Some(r) = self.renderer.as_mut() {
                        r.window().request_redraw();
                    }
                }
                Err(_) => {}
            }
        }

        let completed_scan = self
            .scanner_rx
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some(scan) = completed_scan {
            self.scanner_rx = None;
            self.finish_folder_scan(scan);
        }
    }
}

fn selected_file_index(files: &[PathBuf], selected: &Path) -> Option<usize> {
    files.iter().position(|path| path == selected).or_else(|| {
        let selected_name = selected.file_name()?;
        files
            .iter()
            .position(|path| path.file_name() == Some(selected_name))
    })
}

fn selected_scan_is_current(current: Option<&Path>, selected: &Path) -> bool {
    current == Some(selected)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_load_icon() {
        assert!(load_icon().is_some(), "load_icon returned None!");
    }

    #[test]
    fn selected_file_matches_relative_scan_results_by_name() {
        let files = vec![PathBuf::from("./img1.png"), PathBuf::from("./img2.png")];
        assert_eq!(selected_file_index(&files, Path::new("img2.png")), Some(1));
        assert_eq!(selected_file_index(&files, Path::new("missing.png")), None);
    }

    #[test]
    fn stale_selected_file_scan_is_discarded() {
        let selected = Path::new("selected.png");
        assert!(selected_scan_is_current(Some(selected), selected));
        assert!(!selected_scan_is_current(None, selected));
        assert!(!selected_scan_is_current(
            Some(Path::new("replacement.png")),
            selected
        ));
    }
}
