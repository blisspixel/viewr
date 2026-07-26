//! The application: a message loop of our own on winit's event loop. For Phase 0
//! it opens a window, sets up the GPU renderer, and clears each frame to the
//! theme background. The Elm-style shape (one state, messages, update, render)
//! is borrowed without depending on a UI framework.
#![allow(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::curate::{FlagSet, TrashedFile};
use crate::decode::DecodedImage;
use crate::error::Error;
use crate::gpu::{FrameResult, ImagePreview, Renderer};
use crate::prefetch::{self, PrefetchCache};
use crate::theme::Preference;
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
    run_internal(image_path, None).map(|_| ())
}

/// Run an explicit local-only GUI performance probe and return its measurements.
///
/// The probe opens `image_path`, samples a bounded set of folder positions, then
/// exits automatically. Normal viewer launches never collect these measurements.
///
/// # Errors
/// Returns [`Error`] if the event loop, renderer, image load, or memory sampling
/// prevents the probe from producing a complete report.
pub fn run_performance_probe(
    image_path: PathBuf,
    application_started: Instant,
) -> Result<crate::performance::PerformanceReport, Error> {
    run_internal(
        Some(image_path),
        Some(PerformanceProbe::new(application_started)),
    )?
    .ok_or_else(|| Error::Platform("performance probe ended without a report".into()))
}

fn run_internal(
    image_path: Option<PathBuf>,
    performance_probe: Option<PerformanceProbe>,
) -> Result<Option<crate::performance::PerformanceReport>, Error> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    // A viewer is idle most of the time; wait for events rather than spin.
    event_loop.set_control_flow(ControlFlow::Wait);
    let event_proxy = event_loop.create_proxy();
    if let Some(deadline) = performance_probe.as_ref().map(|probe| probe.deadline) {
        schedule_performance_wake(event_proxy.clone(), "viewr-performance-deadline", deadline)
            .map_err(Error::Platform)?;
    }
    #[cfg(target_os = "macos")]
    crate::macos::install_open_file_handler(event_proxy.clone())
        .map_err(|message| Error::Platform(message.to_owned()))?;
    let (thumb_result_tx, thumb_rx) = mpsc::channel();
    let (prefetch_result_tx, prefetch_rx) = mpsc::channel();
    let image_path = image_path.map(|path| crate::fs::canonical_file_path(&path).unwrap_or(path));
    let probe_enabled = performance_probe.is_some();
    let mut app = App {
        image_path,
        renderer: None,
        playlist: None,
        scanner_rx: None,
        transform: Transform::default(),
        custom_crop_ratio: (3, 5),
        heal: HealTool::default(),
        is_fullscreen: false,
        last_trashed: Vec::new(),
        current_image: None,
        animation: None,
        image_details: None,
        auxiliary_loader_rx: None,
        load_error: None,
        save_worker: None,
        crop_worker: None,
        preview_worker: None,
        loaded_image_path: None,
        show_image_info: false,
        show_tools_panel: false,
        tools_panel_open: true,
        tools_panel_side: crate::ui::DockSide::Left,
        show_filmstrip_panel: probe_enabled,
        filmstrip_panel_open: true,
        image_info_side: crate::ui::DockSide::Right,
        // Privacy default: Save As strips EXIF/GPS unless the user opts in.
        retain_exif: false,
        bg_override: None,
        theme_preference: crate::theme::load_preference(),
        show_about: false,
        image_loader_rx: None,
        image_load_generation: Arc::new(AtomicU64::new(0)),
        resize_on_load: None,
        flags: FlagSet::new(),
        modifiers: ModifiersState::default(),
        toast: None,
        toast_until: None,
        egui_repaint_at: None,
        cursor_pos: (0.0, 0.0),
        last_click: None,
        space_held: false,
        space_dragged: false,
        mouse_left_down: false,
        mouse_right_down: false,
        right_click_start: None,
        context_menu_pos: None,
        thumb_result_tx,
        thumb_rx,
        thumbs_in_flight: HashSet::new(),
        thumb_textures: HashMap::new(),
        prefetch: PrefetchCache::with_limits(
            prefetch::DEFAULT_CAPACITY,
            prefetch::DEFAULT_MAX_BYTES,
        ),
        prefetch_in_flight: HashSet::new(),
        prefetch_result_tx,
        prefetch_rx,
        event_proxy,
        performance_probe,
    };
    if let Some(path) = app.image_path.clone() {
        app.load_and_scan(path);
    }
    event_loop.run_app(&mut app)?;
    let Some(probe) = app.performance_probe else {
        return Ok(None);
    };
    probe
        .outcome
        .unwrap_or_else(|| Err("performance probe exited before completion".into()))
        .map(Some)
        .map_err(Error::Platform)
}

struct Playlist {
    files: Vec<PathBuf>,
    index: usize,
}

enum ScanPurpose {
    SelectedFile(PathBuf),
    OpenFolder,
}

const PERFORMANCE_PROBE_TIMEOUT: Duration = Duration::from_mins(1);
const PERFORMANCE_IDLE_OBSERVATION: Duration = Duration::from_millis(500);

struct PerformanceProbe {
    started_at: Instant,
    deadline: Instant,
    window_ready: Option<Duration>,
    first_pixel: Option<Duration>,
    max_navigation: Duration,
    navigation_started: Option<Instant>,
    navigation_target: Option<PathBuf>,
    navigation_targets: Option<VecDeque<usize>>,
    last_presented_path: Option<PathBuf>,
    idle_until: Option<Instant>,
    idle_redraws: u64,
    peak_resident_bytes: u64,
    outcome: Option<Result<crate::performance::PerformanceReport, String>>,
}

impl PerformanceProbe {
    fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            deadline: started_at + PERFORMANCE_PROBE_TIMEOUT,
            window_ready: None,
            first_pixel: None,
            max_navigation: Duration::ZERO,
            navigation_started: None,
            navigation_target: None,
            navigation_targets: None,
            last_presented_path: None,
            idle_until: None,
            idle_redraws: 0,
            peak_resident_bytes: 0,
            outcome: None,
        }
    }

    fn record_window_ready(&mut self, now: Instant) {
        self.window_ready.get_or_insert(now - self.started_at);
    }

    fn record_presented_image(&mut self, path: &Path, now: Instant) {
        self.first_pixel.get_or_insert(now - self.started_at);
        self.last_presented_path = Some(path.to_owned());
        if self.navigation_target.as_deref() == Some(path)
            && let Some(started) = self.navigation_started.take()
        {
            self.max_navigation = self.max_navigation.max(now - started);
            self.navigation_target = None;
        }
    }

    fn reset_idle_observation(&mut self) {
        self.idle_until = None;
        self.idle_redraws = 0;
    }
}

fn schedule_performance_wake(
    event_proxy: EventLoopProxy<UserEvent>,
    thread_name: &str,
    deadline: Instant,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()));
            let _ = event_proxy.send_event(UserEvent::Wake);
        })
        .map(|_| ())
        .map_err(|error| format!("could not start performance timer: {error}"))
}

/// Application-level events delivered from native platform integrations.
pub(crate) enum UserEvent {
    /// Background work completed and the event loop should poll its channels.
    Wake,
    /// An operating-system assistive technology requested the accessibility
    /// tree or invoked an accessible control.
    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    AccessKit(accesskit_winit::Event),
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

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
impl From<accesskit_winit::Event> for UserEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::AccessKit(event)
    }
}

struct FolderScan {
    purpose: ScanPurpose,
    files: std::io::Result<Vec<PathBuf>>,
}

struct SaveWorker {
    result_rx: Receiver<Result<crate::edit::MetadataDisposition, String>>,
}

struct CropWorker {
    source_path: PathBuf,
    result_rx: Receiver<Result<DecodedImage, String>>,
}

#[derive(Clone, Copy)]
enum PresentationKind {
    Loaded,
    Cropped,
}

struct PreviewWorker {
    path: PathBuf,
    kind: PresentationKind,
    result_rx: Receiver<Result<(Arc<DecodedImage>, ImagePreview), String>>,
}

type AuxiliaryLoadResult = (
    PathBuf,
    Result<Option<crate::animated::DecodedAnimation>, String>,
    crate::image_info::ImageDetails,
);

/// The aspect-ratio constraint to enforce when cropping.
///
/// Fixed ratios are data rather than enum variants so adding a preset never
/// requires another branch in the crop geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CropRatio {
    /// Free-form cropping with no locked ratio.
    Free,
    /// Preserve the current image's original pixel aspect ratio.
    Original,
    /// Lock to an explicit width-to-height ratio.
    Fixed {
        /// Relative width component. Zero is treated as an unlocked ratio.
        width: u16,
        /// Relative height component. Zero is treated as an unlocked ratio.
        height: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl CropRatio {
    /// A square crop.
    pub const SQUARE: Self = Self::fixed(1, 1);
    /// Standard 3:2 landscape photography crop.
    pub const THREE_TWO: Self = Self::fixed(3, 2);
    /// Portrait orientation of [`Self::THREE_TWO`].
    pub const TWO_THREE: Self = Self::fixed(2, 3);
    /// Standard 4:3 landscape crop.
    pub const FOUR_THREE: Self = Self::fixed(4, 3);
    /// Portrait orientation of [`Self::FOUR_THREE`].
    pub const THREE_FOUR: Self = Self::fixed(3, 4);
    /// Common 8x10 and 16x20 landscape print crop.
    pub const FIVE_FOUR: Self = Self::fixed(5, 4);
    /// Portrait orientation of [`Self::FIVE_FOUR`].
    pub const FOUR_FIVE: Self = Self::fixed(4, 5);
    /// Standard 5:3 landscape crop.
    pub const FIVE_THREE: Self = Self::fixed(5, 3);
    /// Portrait orientation of [`Self::FIVE_THREE`].
    pub const THREE_FIVE: Self = Self::fixed(3, 5);
    /// Widescreen landscape crop.
    pub const SIXTEEN_NINE: Self = Self::fixed(16, 9);
    /// Portrait orientation of [`Self::SIXTEEN_NINE`].
    pub const NINE_SIXTEEN: Self = Self::fixed(9, 16);

    /// Construct an explicit width-to-height crop ratio.
    #[must_use]
    pub const fn fixed(width: u16, height: u16) -> Self {
        Self::Fixed { width, height }
    }

    /// Return explicit ratio components, if this is a fixed ratio.
    #[must_use]
    pub const fn components(self) -> Option<(u16, u16)> {
        match self {
            Self::Fixed { width, height } if width != 0 && height != 0 => Some((width, height)),
            Self::Free | Self::Original | Self::Fixed { .. } => None,
        }
    }

    /// A compact label suitable for the crop toolbar.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Free => "Free".to_owned(),
            Self::Original => "Original".to_owned(),
            Self::Fixed { width, height } => format!("{width}:{height}"),
        }
    }
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

const EDIT_HISTORY_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_HEAL_BRUSH_RADIUS: u32 = 18;

struct HealTool {
    active: bool,
    brush_radius: u32,
    feather_percent: u8,
    stroke: Vec<crate::heal::StrokePoint>,
    painting: bool,
    worker: Option<HealWorker>,
    refresh: Option<HealRefresh>,
    history: crate::heal::PatchHistory,
}

struct HealWorker {
    result_rx: Receiver<HealWorkerOutput>,
    cancel: Arc<AtomicBool>,
    apply_result: bool,
    replacing_latest: bool,
    previous_candidate_index: usize,
}

struct HealWorkerOutput {
    result: Result<crate::heal::SpotHealResult, String>,
    job: Option<crate::heal::SpotHealJob>,
}

struct HealRefresh {
    job: crate::heal::SpotHealJob,
    candidate_index: usize,
    candidate_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HealStrokeUpdate {
    Added,
    Unchanged,
    LeftImage,
    TooManyPoints,
}

fn append_heal_stroke_point(
    stroke: &mut Vec<crate::heal::StrokePoint>,
    point: Option<crate::heal::StrokePoint>,
    brush_radius: u32,
) -> HealStrokeUpdate {
    let Some(point) = point else {
        return HealStrokeUpdate::LeftImage;
    };
    let spacing = (brush_radius as f32 * 0.2).max(1.0);
    if stroke
        .last()
        .is_some_and(|last| (point.x - last.x).hypot(point.y - last.y) < spacing)
    {
        return HealStrokeUpdate::Unchanged;
    }
    if stroke.len() >= crate::heal::MAX_STROKE_POINTS {
        return HealStrokeUpdate::TooManyPoints;
    }
    stroke.push(point);
    HealStrokeUpdate::Added
}

impl Default for HealTool {
    fn default() -> Self {
        Self {
            active: false,
            brush_radius: DEFAULT_HEAL_BRUSH_RADIUS,
            feather_percent: crate::heal::DEFAULT_FEATHER_PERCENT,
            stroke: Vec::new(),
            painting: false,
            worker: None,
            refresh: None,
            history: crate::heal::PatchHistory::new(EDIT_HISTORY_BYTES),
        }
    }
}

impl HealTool {
    fn reset_for_image(&mut self) {
        self.cancel_worker();
        self.stroke.clear();
        self.painting = false;
        self.refresh = None;
        self.history.clear();
    }

    fn is_busy(&self) -> bool {
        self.worker.is_some()
    }

    fn cancel_worker(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            worker.apply_result = false;
            worker.cancel.store(true, Ordering::Relaxed);
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
    /// Last custom crop ratio entered during this process session.
    custom_crop_ratio: (u16, u16),
    heal: HealTool,
    is_fullscreen: bool,
    last_trashed: Vec<TrashedFile>,
    current_image: Option<Arc<DecodedImage>>,
    /// Timed frames for the current GIF, WebP, or APNG.
    animation: Option<crate::animated::AnimationPlayback>,
    /// Best-effort facts for the current Image Information panel.
    image_details: Option<crate::image_info::ImageDetails>,
    /// Replace-latest animation and metadata result for the current source.
    auxiliary_loader_rx: Option<Receiver<AuxiliaryLoadResult>>,
    /// Most recent foreground decode failure for the selected path.
    load_error: Option<String>,
    /// At most one explicit Save As encode running off the UI thread.
    save_worker: Option<SaveWorker>,
    /// At most one full-resolution crop running off the UI thread.
    crop_worker: Option<CropWorker>,
    /// Replace-latest over-limit preview prepared outside the event thread.
    preview_worker: Option<PreviewWorker>,
    /// Source path corresponding exactly to `current_image` and the GPU texture.
    loaded_image_path: Option<PathBuf>,
    show_image_info: bool,
    /// Whether the tools dock reserves any viewport space.
    show_tools_panel: bool,
    /// Whether the docked tools panel is expanded.
    tools_panel_open: bool,
    /// Horizontal edge used by the tools dock.
    tools_panel_side: crate::ui::DockSide,
    /// Whether folder previews reserve any viewport space.
    show_filmstrip_panel: bool,
    /// Whether the docked folder-preview panel is expanded.
    filmstrip_panel_open: bool,
    /// Horizontal edge used by Image Information.
    image_info_side: crate::ui::DockSide,
    /// When true, Save As copies EXIF from the source. Default **false** (strip).
    retain_exif: bool,
    bg_override: Option<[f64; 4]>,
    /// Persisted application-chrome and default-canvas appearance.
    theme_preference: Preference,
    /// Whether the accessible About window is open.
    show_about: bool,
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
    /// Deadline requested by egui for delayed UI state such as tooltips.
    egui_repaint_at: Option<Instant>,
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
    /// Right mouse button currently down.
    mouse_right_down: bool,
    /// Position where right click started.
    right_click_start: Option<(f64, f64)>,
    /// Screen position of the right-click context menu, if open.
    context_menu_pos: Option<[f32; 2]>,
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
    /// Wakes the event loop when background work finishes before a window exists.
    event_proxy: EventLoopProxy<UserEvent>,
    /// Explicit developer/CI performance probe; absent from normal launches.
    performance_probe: Option<PerformanceProbe>,
}

fn primary_modifier_pressed(modifiers: ModifiersState) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.super_key()
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control_key()
    }
}

fn single_key_shortcut_allowed(modifiers: ModifiersState) -> bool {
    !modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key()
}

fn route_consumed_keyboard_key(
    key: &winit::keyboard::Key,
    is_cropping: bool,
    is_healing: bool,
) -> bool {
    use winit::keyboard::{Key, NamedKey};

    match key {
        Key::Character(character) => {
            let character = character.as_str();
            matches!(character, "0" | "1" | "+" | "=" | "-" | "_" | "/")
                || [
                    "o", "t", "g", "i", "r", "l", "h", "v", "s", "c", "j", "u", "x", "b", "f", "z",
                    "y",
                ]
                .iter()
                .any(|shortcut| character.eq_ignore_ascii_case(shortcut))
        }
        Key::Named(NamedKey::ArrowRight | NamedKey::ArrowLeft | NamedKey::F5) => true,
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp) => is_cropping,
        Key::Named(NamedKey::Escape) => is_cropping || is_healing,
        _ => false,
    }
}

fn image_is_fully_displayed(source: Option<(u32, u32)>, displayed: Option<(u32, u32)>) -> bool {
    source.is_some() && source == displayed
}

fn validate_performance_report(
    report: crate::performance::PerformanceReport,
) -> Result<crate::performance::PerformanceReport, String> {
    if report.decoded_cache_entries > prefetch::DEFAULT_CAPACITY {
        return Err(format!(
            "decoded cache retained {} entries; limit is {}",
            report.decoded_cache_entries,
            prefetch::DEFAULT_CAPACITY
        ));
    }
    if report.decoded_cache_bytes > u64::try_from(prefetch::DEFAULT_MAX_BYTES).unwrap_or(u64::MAX) {
        return Err(format!(
            "decoded cache retained {} bytes; limit is {}",
            report.decoded_cache_bytes,
            prefetch::DEFAULT_MAX_BYTES
        ));
    }
    if report.thumbnail_texture_entries > 9 {
        return Err(format!(
            "thumbnail cache retained {} entries; limit is 9",
            report.thumbnail_texture_entries
        ));
    }
    Ok(report)
}

impl App {
    fn open_file_request(&mut self, path: PathBuf) {
        self.load_and_scan(path);
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
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
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
        let event_proxy = self.event_proxy.clone();
        let spawn_result = std::thread::Builder::new()
            .name("viewr-folder-scan".into())
            .spawn(move || {
                let files = crate::fs::scan_images(&directory);
                let _ = sender.send(FolderScan { purpose, files });
                let _ = event_proxy.send_event(UserEvent::Wake);
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
        self.thumb_textures.clear();
        self.playlist = Some(Playlist { files, index });
    }

    fn finish_folder_scan(&mut self, scan: FolderScan) -> bool {
        if let ScanPurpose::SelectedFile(selected) = &scan.purpose
            && !selected_scan_is_current(self.image_path.as_deref(), selected)
        {
            return false;
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
        true
    }

    fn display_loaded_image(&mut self, path: &Path, image: DecodedImage) {
        self.present_image(path, Arc::new(image), PresentationKind::Loaded);
    }

    fn present_image(&mut self, path: &Path, image: Arc<DecodedImage>, kind: PresentationKind) {
        let required = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.required_preview(&image));
        let Some(spec) = required else {
            self.finish_image_presentation(path, image, None, kind);
            return;
        };

        let generation = self.image_load_generation.load(Ordering::Acquire);
        let current_generation = Arc::clone(&self.image_load_generation);
        let worker_image = Arc::clone(&image);
        let worker_path = path.to_owned();
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let scheduled = crate::decode::schedule_image_preview(move || {
            let result = crate::gpu::prepare_image_preview(&worker_image, spec, || {
                current_generation.load(Ordering::Acquire) != generation
            });
            match result {
                Ok(Some(preview)) => {
                    let _ = sender.send(Ok((worker_image, preview)));
                    let _ = event_proxy.send_event(UserEvent::Wake);
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    let _ = event_proxy.send_event(UserEvent::Wake);
                }
            }
        });
        match scheduled {
            Ok(()) => {
                self.preview_worker = Some(PreviewWorker {
                    path: worker_path,
                    kind,
                    result_rx: receiver,
                });
                self.show_toast("Preparing a display-sized preview in the background");
            }
            Err(error) => {
                log::error!("failed to queue image preview");
                self.show_toast(format!("Could not prepare image preview: {error}"));
            }
        }
    }

    fn finish_image_presentation(
        &mut self,
        path: &Path,
        image: Arc<DecodedImage>,
        preview: Option<&ImagePreview>,
        kind: PresentationKind,
    ) {
        let should_resize = self.resize_on_load.as_deref() == Some(path);
        let full_resolution = if let Some(renderer) = self.renderer.as_mut() {
            match renderer.set_image(&image, preview) {
                Ok(full_resolution) => {
                    if should_resize {
                        resize_window_to_image(renderer);
                    }
                    full_resolution
                }
                Err(error) => {
                    log::error!("failed to upload prepared image");
                    self.show_toast(format!("Could not display image: {error}"));
                    return;
                }
            }
        } else {
            true
        };
        if should_resize && self.renderer.is_some() {
            self.resize_on_load = None;
        }
        self.current_image = Some(image);
        self.loaded_image_path = Some(path.to_owned());
        self.load_error = None;
        match kind {
            PresentationKind::Loaded => self.start_auxiliary_load(path),
            PresentationKind::Cropped => {
                self.heal.reset_for_image();
                self.show_toast("Crop applied");
            }
        }
        if !full_resolution {
            self.show_toast(
                "Full image shown as a GPU-limited preview; export remains full resolution",
            );
        }
    }

    fn poll_preview_result(&mut self) {
        let completed = self.preview_worker.as_ref().and_then(|worker| {
            worker
                .result_rx
                .try_recv()
                .ok()
                .map(|result| (worker.path.clone(), worker.kind, result))
        });
        let Some((path, kind, result)) = completed else {
            return;
        };
        self.preview_worker = None;
        if self.image_path.as_ref() != Some(&path) {
            return;
        }
        match result {
            Ok((image, preview)) => {
                self.finish_image_presentation(&path, image, Some(&preview), kind);
                self.request_redraw();
            }
            Err(error) => self.show_toast(format!("Could not prepare image preview: {error}")),
        }
    }

    fn invalidate_displayed_image(&mut self) {
        self.heal.reset_for_image();
        self.crop_worker = None;
        self.preview_worker = None;
        self.animation = None;
        self.image_details = None;
        self.auxiliary_loader_rx = None;
        self.load_error = None;
        self.current_image = None;
        self.loaded_image_path = None;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_image();
        }
    }

    /// Stop work and edit state tied to the old image while leaving its last
    /// good pixels on screen until a replacement has decoded successfully.
    fn prepare_for_image_load(&mut self) {
        self.heal.reset_for_image();
        self.crop_worker = None;
        self.preview_worker = None;
        self.animation = None;
        self.auxiliary_loader_rx = None;
        self.load_error = None;
    }

    fn start_auxiliary_load(&mut self, path: &Path) {
        self.animation = None;
        self.image_details = None;
        self.auxiliary_loader_rx = None;
        let path = path.to_owned();
        let job_path = path.clone();
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let current_generation = Arc::clone(&self.image_load_generation);
        let generation = current_generation.load(Ordering::Acquire);
        let scheduled = crate::decode::schedule_current_image_details(move || {
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let animation = crate::animated::DecodedAnimation::load_background_if_current(
                &job_path,
                &current_generation,
                generation,
            )
            .map_err(|error| error.to_string());
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let details = crate::image_info::ImageDetails::load(&job_path);
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let _ = sender.send((job_path, animation, details));
            let _ = event_proxy.send_event(UserEvent::Wake);
        });
        match scheduled {
            Ok(()) => self.auxiliary_loader_rx = Some(receiver),
            Err(error) => {
                log::error!("failed to queue current-image details");
                self.show_toast(format!("Image details unavailable: {error}"));
            }
        }
    }

    fn poll_auxiliary_load(&mut self) {
        let completed = self
            .auxiliary_loader_rx
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        let Some((path, result, details)) = completed else {
            return;
        };
        self.auxiliary_loader_rx = None;
        if self.loaded_image_path.as_ref() != Some(&path) || self.image_path.as_ref() != Some(&path)
        {
            return;
        }
        self.image_details = Some(details);
        match result {
            Ok(Some(animation)) => {
                let mut playback =
                    crate::animated::AnimationPlayback::new(animation, Instant::now());
                if self.transform.is_cropping || self.heal.active {
                    playback.pause();
                }
                let image = playback.current_image();
                if let Err(error) = self.upload_realtime_image(&image) {
                    self.show_toast(format!(
                        "Animation unavailable; showing first frame: {error}"
                    ));
                    return;
                }
                self.current_image = Some(image);
                self.animation = Some(playback);
            }
            Ok(None) => {}
            Err(error) => {
                log::debug!("animation playback unavailable");
                self.show_toast(format!(
                    "Animation unavailable; showing first frame: {error}"
                ));
            }
        }
        self.request_redraw();
    }

    fn advance_animation(&mut self, now: Instant) {
        let image = self
            .animation
            .as_mut()
            .and_then(|playback| playback.advance(now).then(|| playback.current_image()));
        let Some(image) = image else {
            return;
        };
        if let Err(error) = self.upload_realtime_image(&image) {
            self.animation = None;
            self.show_toast(format!("Animation stopped: {error}"));
            return;
        }
        self.current_image = Some(image);
    }

    fn toggle_animation_playback(&mut self) {
        let image = self.animation.as_mut().map(|playback| {
            playback.toggle(Instant::now());
            playback.current_image()
        });
        let Some(image) = image else {
            return;
        };
        if let Err(error) = self.upload_realtime_image(&image) {
            self.animation = None;
            self.show_toast(format!("Animation stopped: {error}"));
            return;
        }
        self.current_image = Some(image);
    }

    fn upload_realtime_image(&mut self, image: &DecodedImage) -> Result<(), String> {
        let Some(renderer) = self.renderer.as_mut() else {
            return Ok(());
        };
        if renderer.required_preview(image).is_some() {
            return Err("frames exceed the GPU preview limit".into());
        }
        renderer
            .set_image(image, None)
            .map_err(|error| error.to_string())?;
        renderer.window().request_redraw();
        Ok(())
    }

    fn pause_animation(&mut self) {
        if let Some(playback) = self.animation.as_mut() {
            playback.pause();
        }
    }

    fn discard_animation_for_pixel_edit(&mut self) {
        self.animation = None;
        self.auxiliary_loader_rx = None;
    }

    fn cancel_pending_image_load(&mut self) {
        self.image_load_generation.fetch_add(1, Ordering::AcqRel);
        self.image_loader_rx = None;
        self.preview_worker = None;
        self.resize_on_load = None;
        self.load_error = None;
    }

    fn retry_current_image_load(&mut self) {
        let Some(path) = self.image_path.clone() else {
            return;
        };
        self.spawn_image_load(path);
        self.request_redraw();
    }

    fn reload_current_image(&mut self) {
        if self.heal.is_busy() || self.heal.painting {
            self.show_toast("Wait for spot heal to finish before reloading");
            return;
        }
        if self.crop_worker.is_some() {
            self.show_toast("Wait for the crop to finish before reloading");
            return;
        }
        if self.save_worker.is_some() {
            self.show_toast("Wait for Save As to finish before reloading");
            return;
        }
        if self.image_loader_rx.is_some() || self.preview_worker.is_some() {
            self.show_toast("An image is already loading");
            return;
        }
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };

        // A reload is an explicit disk refresh. Drop any speculative copy and
        // stale filmstrip texture, then keep the last good pixels presented
        // while the replacement decodes in the foreground.
        let _ = self.prefetch.take(&path);
        self.thumb_textures.remove(&path);
        self.transform = Transform::default();
        self.spawn_image_load(path);
        self.show_toast("Reloading file from disk");
        self.request_redraw();
    }

    fn current_loaded_path(&self) -> Option<&Path> {
        let path = self.image_path.as_deref()?;
        (self.current_image.is_some() && self.loaded_image_path.as_deref() == Some(path))
            .then_some(path)
    }

    fn viewport_insets(&self) -> crate::view::ViewportInsets {
        let scale_factor = self
            .renderer
            .as_ref()
            .map_or(1.0, |renderer| renderer.window().scale_factor());
        let has_filmstrip = self
            .playlist
            .as_ref()
            .is_some_and(|playlist| playlist.files.len() > 1);
        let has_image = self
            .renderer
            .as_ref()
            .and_then(Renderer::image_size)
            .is_some();
        crate::ui::viewport_insets(crate::ui::ChromeLayout {
            tools: if !has_image || !self.show_tools_panel {
                crate::ui::DockState::Hidden
            } else if self.tools_panel_open {
                crate::ui::DockState::Expanded
            } else {
                crate::ui::DockState::Collapsed
            },
            tools_side: self.tools_panel_side,
            heal: has_image && self.heal.active,
            filmstrip: if !has_image || !has_filmstrip || !self.show_filmstrip_panel {
                crate::ui::DockState::Hidden
            } else if self.filmstrip_panel_open {
                crate::ui::DockState::Expanded
            } else {
                crate::ui::DockState::Collapsed
            },
            image_info: (has_image && self.show_image_info).then_some(self.image_info_side),
            scale_factor,
        })
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
        let mut p = crate::view::fit_to_viewport(
            (win_size.width, win_size.height),
            image_size,
            rotated90,
            self.viewport_insets(),
        );

        p.scale[0] *= self.transform.zoom;
        p.scale[1] *= self.transform.zoom;
        p.offset[0] += self.transform.offset_x;
        p.offset[1] += self.transform.offset_y;

        let corner_x = (ndc_x - p.offset[0]) / p.scale[0];
        let corner_y = (ndc_y - p.offset[1]) / p.scale[1];

        let base_uv_x = (corner_x + 1.0) * 0.5;
        let base_uv_y = (1.0 - corner_y) * 0.5;

        let cx = base_uv_x - 0.5;
        let cy = base_uv_y - 0.5;

        let uv_matrix = crate::view::uv_transform(
            self.transform.rotation_steps,
            self.transform.flip_h,
            self.transform.flip_v,
        );

        let uv_x = uv_matrix[0] * cx + uv_matrix[2] * cy + 0.5;
        let uv_y = uv_matrix[1] * cx + uv_matrix[3] * cy + 0.5;

        Some((uv_x, uv_y))
    }

    fn toggle_fullscreen(&mut self) {
        self.is_fullscreen = !self.is_fullscreen;
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        if self.is_fullscreen {
            renderer
                .window()
                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        } else {
            renderer.window().set_fullscreen(None);
        }
    }

    fn request_redraw(&self) {
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn rotate_current(&mut self, quarter_turns: i32) {
        if self.current_loaded_path().is_some()
            && self.crop_worker.is_none()
            && self.save_worker.is_none()
        {
            self.transform.rotation_steps += quarter_turns;
            if self.transform.is_cropping
                && let Some(image_size) = self.renderer.as_ref().and_then(Renderer::image_size)
            {
                let ratio =
                    crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
                let current = self
                    .transform
                    .crop_rect
                    .unwrap_or_else(|| default_crop_rect(image_size, ratio));
                self.transform.crop_rect = Some(fit_crop_rect_to_ratio(current, image_size, ratio));
            }
            self.request_redraw();
        }
    }

    fn flip_current_horizontally(&mut self) {
        if self.current_loaded_path().is_some()
            && self.crop_worker.is_none()
            && self.save_worker.is_none()
        {
            self.transform.flip_h = !self.transform.flip_h;
            self.request_redraw();
        }
    }

    fn flip_current_vertically(&mut self) {
        if self.current_loaded_path().is_some()
            && self.crop_worker.is_none()
            && self.save_worker.is_none()
        {
            self.transform.flip_v = !self.transform.flip_v;
            self.request_redraw();
        }
    }

    fn handle_single_key_shortcut(&mut self, key: &str) {
        match key {
            "o" | "O" => self.open_image_dialog(),
            "t" | "T" => {
                self.show_tools_panel = !self.show_tools_panel;
                self.request_redraw();
            }
            "g" | "G"
                if self
                    .playlist
                    .as_ref()
                    .is_some_and(|playlist| playlist.files.len() > 1) =>
            {
                self.show_filmstrip_panel = !self.show_filmstrip_panel;
                self.request_redraw();
            }
            "i" | "I" => {
                self.show_image_info = !self.show_image_info;
                self.request_redraw();
            }
            "r" | "R" => {
                self.rotate_current(1);
            }
            "l" | "L" => {
                self.rotate_current(-1);
            }
            "h" | "H" => {
                self.flip_current_horizontally();
            }
            "v" | "V" => {
                self.flip_current_vertically();
            }
            "c" | "C" => self.toggle_crop_mode(),
            "j" | "J" => self.toggle_heal_mode(),
            "/" if self.heal.active => self.refresh_heal_source(),
            "u" | "U" => self.undo_trash(),
            "x" | "X" if self.transform.is_cropping => self.swap_crop_ratio(),
            "x" | "X" => self.toggle_flag_current(),
            "b" | "B" => self.trash_flagged(),
            "f" | "F" => self.toggle_fullscreen(),
            "0" => self.fit_to_view(),
            "1" => self.set_actual_size(),
            "+" | "=" => self.zoom_at_viewport_center(1.15),
            "-" | "_" => self.zoom_at_viewport_center(1.0 / 1.15),
            _ => {}
        }
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
            let event_proxy = self.event_proxy.clone();
            let scheduled = crate::decode::schedule_background_decode(move || {
                let res = DecodedImage::load_background(&job_path).map_err(|e| e.to_string());
                let _ = tx.send((job_path, res));
                let _ = event_proxy.send_event(UserEvent::Wake);
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
            if self.show_filmstrip_panel && self.filmstrip_panel_open {
                self.request_thumbs_for_filmstrip();
            }
        }
    }

    fn toggle_flag_current(&mut self) {
        if self.crop_worker.is_some() || self.save_worker.is_some() || self.preview_worker.is_some()
        {
            return;
        }
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
        if self.crop_worker.is_some() || self.save_worker.is_some() || self.preview_worker.is_some()
        {
            return;
        }
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
        if self.crop_worker.is_some()
            || self.save_worker.is_some()
            || self.preview_worker.is_some()
            || self.heal.is_busy()
        {
            return;
        }
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

    fn toggle_crop_mode(&mut self) {
        if self.preview_worker.is_some() {
            self.show_toast("Wait for the image preview to finish before cropping");
            return;
        }
        if self.current_loaded_path().is_none() {
            return;
        }
        if self.crop_worker.is_some() {
            self.show_toast("A crop is already being applied");
            return;
        }
        if self.save_worker.is_some() {
            self.show_toast("Wait for the current save to finish before cropping");
            return;
        }
        if self.transform.is_cropping {
            self.cancel_crop();
            return;
        }
        if self.heal.is_busy() {
            self.show_toast("Spot heal is still finishing");
            return;
        }

        self.heal.active = false;
        self.heal.stroke.clear();
        self.heal.painting = false;
        self.heal.cancel_worker();
        self.pause_animation();
        self.transform.is_cropping = true;
        self.transform.crop_start = None;
        let ratio = crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
        self.transform.crop_rect = self
            .renderer
            .as_ref()
            .and_then(Renderer::image_size)
            .map(|image_size| default_crop_rect(image_size, ratio));
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn set_crop_ratio(&mut self, ratio: CropRatio) {
        self.transform.crop_ratio = ratio;
        if self.transform.is_cropping
            && let Some(image_size) = self.renderer.as_ref().and_then(Renderer::image_size)
        {
            let source_ratio = crop_ratio_for_source(ratio, self.transform.rotation_steps);
            let current = self
                .transform
                .crop_rect
                .unwrap_or_else(|| default_crop_rect(image_size, CropRatio::Free));
            self.transform.crop_rect =
                Some(fit_crop_rect_to_ratio(current, image_size, source_ratio));
        }
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn swap_crop_ratio(&mut self) {
        let swapped = match self.transform.crop_ratio {
            CropRatio::Original => {
                let Some((width, height)) = self
                    .renderer
                    .as_ref()
                    .and_then(Renderer::image_size)
                    .and_then(|(width, height)| reduced_crop_ratio(height, width))
                else {
                    return;
                };
                CropRatio::fixed(width, height)
            }
            CropRatio::Fixed { width, height } if width != 0 && height != 0 => {
                CropRatio::fixed(height, width)
            }
            CropRatio::Free | CropRatio::Fixed { .. } => return,
        };
        self.set_crop_ratio(swapped);
    }

    fn move_crop_from_logical_pointer(&mut self, pointer: [f32; 2], delta: [f32; 2]) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let scale = renderer.window().scale_factor() as f32;
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let previous = [pointer[0] - delta[0], pointer[1] - delta[1]];
        let Some((previous_u, previous_v)) = self.screen_to_uv(
            f64::from(previous[0] * scale),
            f64::from(previous[1] * scale),
        ) else {
            return;
        };
        let Some((pointer_u, pointer_v)) =
            self.screen_to_uv(f64::from(pointer[0] * scale), f64::from(pointer[1] * scale))
        else {
            return;
        };
        let Some(image_size) = self.renderer.as_ref().and_then(Renderer::image_size) else {
            return;
        };
        let ratio = crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
        let current = self
            .transform
            .crop_rect
            .unwrap_or_else(|| default_crop_rect(image_size, ratio));
        self.transform.crop_rect = Some(adjust_crop_rect(
            current,
            image_size,
            ratio,
            pointer_u - previous_u,
            pointer_v - previous_v,
            false,
        ));
        self.request_redraw();
    }

    fn resize_crop_from_logical_pointer(&mut self, handle_center: [f32; 2], pointer: [f32; 2]) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let scale = renderer.window().scale_factor() as f32;
        let Some(image_size) = renderer.image_size() else {
            return;
        };
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let Some(handle_uv) = self.screen_to_uv(
            f64::from(handle_center[0] * scale),
            f64::from(handle_center[1] * scale),
        ) else {
            return;
        };
        let Some(pointer_uv) =
            self.screen_to_uv(f64::from(pointer[0] * scale), f64::from(pointer[1] * scale))
        else {
            return;
        };
        let ratio = crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
        let current = self
            .transform
            .crop_rect
            .unwrap_or_else(|| default_crop_rect(image_size, ratio));
        let handle = crop_handle_from_uv(current, handle_uv);
        self.transform.crop_rect = Some(resize_crop_rect_from_pointer(
            current, image_size, ratio, handle, pointer_uv,
        ));
        self.request_redraw();
    }

    fn toggle_heal_mode(&mut self) {
        if self.current_loaded_path().is_none() {
            return;
        }
        if !self.heal.active && self.crop_worker.is_some() {
            self.show_toast("Wait for the crop to finish before using Spot Heal");
            return;
        }
        if !self.heal.active && self.preview_worker.is_some() {
            self.show_toast("Wait for the image preview to finish before using Spot Heal");
            return;
        }
        if !self.heal.active && self.save_worker.is_some() {
            self.show_toast("Wait for the current save to finish before using Spot Heal");
            return;
        }
        if !self.heal.active && self.heal.is_busy() {
            self.show_toast("Spot heal is still finishing");
            return;
        }
        if !self.heal.active && !self.can_heal_current_image() {
            self.show_toast(
                "Spot Heal is unavailable for images larger than the GPU texture limit",
            );
            return;
        }
        if self.heal.active {
            if self.heal.painting {
                self.finish_heal_stroke();
            }
            self.heal.active = false;
            self.heal.stroke.clear();
            self.heal.painting = false;
            if self.heal.is_busy() {
                self.show_toast("Finishing spot heal in memory");
            }
        } else {
            self.pause_animation();
            self.heal.active = true;
            self.heal.stroke.clear();
            self.heal.painting = false;
            self.cancel_crop();
            self.show_tools_panel = true;
            self.tools_panel_open = true;
        }
        self.request_redraw();
    }

    fn can_heal_current_image(&self) -> bool {
        image_is_fully_displayed(
            self.current_image
                .as_ref()
                .map(|image| (image.width, image.height)),
            self.renderer
                .as_ref()
                .and_then(Renderer::image_texture_size),
        )
    }

    fn set_heal_brush_radius(&mut self, radius: u32) {
        self.heal.brush_radius =
            radius.clamp(crate::heal::MIN_BRUSH_RADIUS, crate::heal::MAX_BRUSH_RADIUS);
        self.request_redraw();
    }

    fn set_heal_feather(&mut self, percent: u8) {
        self.heal.feather_percent = percent.min(crate::heal::MAX_FEATHER_PERCENT);
        self.request_redraw();
    }

    fn refresh_heal_source(&mut self) {
        if self.heal.is_busy()
            || self.crop_worker.is_some()
            || self.save_worker.is_some()
            || self.preview_worker.is_some()
        {
            return;
        }
        let Some(refresh) = self.heal.refresh.as_ref() else {
            return;
        };
        if refresh.candidate_count < 2 {
            return;
        }
        let previous_candidate_index = refresh.candidate_index;
        let candidate_index = (previous_candidate_index + 1) % refresh.candidate_count;
        let job = refresh.job.clone();
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let spawn = std::thread::Builder::new()
            .name("viewr-spot-heal-refresh".into())
            .spawn(move || {
                let result = job
                    .run_ranked_cancellable(candidate_index, &worker_cancel)
                    .map_err(|error| error.to_string());
                let _ = sender.send(HealWorkerOutput {
                    result,
                    job: Some(job),
                });
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
        match spawn {
            Ok(_) => {
                self.heal.worker = Some(HealWorker {
                    result_rx: receiver,
                    cancel,
                    apply_result: true,
                    replacing_latest: true,
                    previous_candidate_index,
                });
                self.request_redraw();
            }
            Err(error) => self.show_toast(format!("Could not refresh heal source: {error}")),
        }
    }

    fn heal_point_at(&self, screen: (f64, f64)) -> Option<crate::heal::StrokePoint> {
        let renderer = self.renderer.as_ref()?;
        let image = self.current_image.as_ref()?;
        if renderer.image_texture_size() != Some((image.width, image.height)) {
            return None;
        }
        let size = renderer.window().inner_size();
        let viewport =
            crate::view::safe_viewport_rect((size.width, size.height), self.viewport_insets())?;
        let left = f64::from(viewport.x);
        let top = f64::from(viewport.y);
        let right = left + f64::from(viewport.width);
        let bottom = top + f64::from(viewport.height);
        if !screen.0.is_finite()
            || !screen.1.is_finite()
            || screen.0 < left
            || screen.0 >= right
            || screen.1 < top
            || screen.1 >= bottom
        {
            return None;
        }
        let (u, v) = self.screen_to_uv(screen.0, screen.1)?;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        Some(crate::heal::StrokePoint {
            x: (u * image.width as f32).clamp(0.0, image.width.saturating_sub(1) as f32),
            y: (v * image.height as f32).clamp(0.0, image.height.saturating_sub(1) as f32),
        })
    }

    fn begin_heal_stroke(&mut self) {
        if !self.heal.active
            || self.heal.is_busy()
            || self.crop_worker.is_some()
            || self.save_worker.is_some()
            || self.preview_worker.is_some()
        {
            return;
        }
        self.heal.stroke.clear();
        let Some(point) = self.heal_point_at(self.cursor_pos) else {
            self.heal.painting = false;
            return;
        };
        self.discard_animation_for_pixel_edit();
        self.heal.stroke.push(point);
        self.heal.painting = true;
        self.request_redraw();
    }

    fn continue_heal_stroke(&mut self) {
        if !self.heal.painting || self.heal.is_busy() {
            return;
        }
        let point = self.heal_point_at(self.cursor_pos);
        match append_heal_stroke_point(&mut self.heal.stroke, point, self.heal.brush_radius) {
            HealStrokeUpdate::Added => self.request_redraw(),
            HealStrokeUpdate::Unchanged => {}
            HealStrokeUpdate::LeftImage => self.finish_heal_stroke(),
            HealStrokeUpdate::TooManyPoints => {
                self.heal.painting = false;
                self.heal.stroke.clear();
                self.show_toast("Spot-heal stroke is too long; use shorter strokes");
                self.request_redraw();
            }
        }
    }

    fn finish_heal_stroke(&mut self) {
        if !self.heal.painting {
            return;
        }
        self.heal.painting = false;
        let Some(image) = self.current_image.as_ref().map(Arc::clone) else {
            self.heal.stroke.clear();
            return;
        };
        let points = std::mem::take(&mut self.heal.stroke);
        let brush_radius = self.heal.brush_radius;
        let feather_percent = self.heal.feather_percent;
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let spawn = std::thread::Builder::new()
            .name("viewr-spot-heal".into())
            .spawn(move || {
                let output = if worker_cancel.load(Ordering::Relaxed) {
                    HealWorkerOutput {
                        result: Err(crate::heal::HealError::Cancelled.to_string()),
                        job: None,
                    }
                } else {
                    let prepared = crate::heal::SpotHealJob::prepare_with_feather(
                        image.as_ref(),
                        &points,
                        brush_radius,
                        feather_percent,
                    );
                    drop(image);
                    match prepared {
                        Ok(Some(job)) => HealWorkerOutput {
                            result: job
                                .run_ranked_cancellable(0, &worker_cancel)
                                .map_err(|error| error.to_string()),
                            job: Some(job),
                        },
                        Ok(None) => HealWorkerOutput {
                            result: Err(crate::heal::HealError::InvalidStroke.to_string()),
                            job: None,
                        },
                        Err(error) => HealWorkerOutput {
                            result: Err(error.to_string()),
                            job: None,
                        },
                    }
                };
                let _ = sender.send(output);
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
        match spawn {
            Ok(_) => {
                self.heal.worker = Some(HealWorker {
                    result_rx: receiver,
                    cancel,
                    apply_result: true,
                    replacing_latest: false,
                    previous_candidate_index: 0,
                });
                self.request_redraw();
            }
            Err(error) => {
                self.heal.stroke.clear();
                self.show_toast(format!("Could not start spot heal: {error}"));
            }
        }
    }

    fn poll_heal_result(&mut self) {
        use std::sync::mpsc::TryRecvError;

        let polled = self.heal.worker.as_ref().map(|worker| {
            (
                worker.result_rx.try_recv(),
                worker.apply_result,
                worker.replacing_latest,
                worker.previous_candidate_index,
            )
        });
        let (output, apply_result, replacing_latest, previous_candidate_index) = match polled {
            Some((Ok(output), apply_result, replacing_latest, previous_candidate_index)) => (
                Some(output),
                apply_result,
                replacing_latest,
                previous_candidate_index,
            ),
            Some((Err(TryRecvError::Disconnected), apply_result, _, _)) => {
                self.heal.worker = None;
                self.heal.stroke.clear();
                if apply_result {
                    self.show_toast("Spot heal stopped unexpectedly");
                }
                return;
            }
            Some((Err(TryRecvError::Empty), _, _, _)) | None => (None, false, false, 0),
        };
        let Some(output) = output else {
            return;
        };
        self.heal.worker = None;
        self.heal.stroke.clear();
        if !apply_result {
            self.request_redraw();
            return;
        }
        match output.result {
            Ok(result) => {
                let patch = result.patch;
                let apply_result = self
                    .current_image
                    .as_mut()
                    .and_then(Arc::get_mut)
                    .ok_or(crate::heal::HealError::InvalidImageBuffer)
                    .and_then(|image| crate::heal::apply_patch(image, &patch));
                match apply_result {
                    Ok(inverse) => {
                        if !replacing_latest {
                            self.heal.history.record(inverse);
                        }
                        self.heal.refresh = output.job.and_then(|job| {
                            (result.candidate_count > 1).then_some(HealRefresh {
                                job,
                                candidate_index: result.candidate_index,
                                candidate_count: result.candidate_count,
                            })
                        });
                        self.update_rendered_patch(&patch);
                        self.show_toast(if replacing_latest {
                            format!(
                                "Heal source {} of {}",
                                result.candidate_index + 1,
                                result.candidate_count
                            )
                        } else {
                            "Spot healed. Undo is available.".to_owned()
                        });
                    }
                    Err(error) => {
                        if replacing_latest && let Some(refresh) = self.heal.refresh.as_mut() {
                            refresh.candidate_index = previous_candidate_index;
                        }
                        self.show_toast(format!("Spot heal failed: {error}"));
                    }
                }
            }
            Err(error) => {
                if replacing_latest && let Some(refresh) = self.heal.refresh.as_mut() {
                    refresh.candidate_index = previous_candidate_index;
                }
                self.show_toast(format!("Spot heal failed: {error}"));
            }
        }
        self.request_redraw();
    }

    fn undo_edit(&mut self) {
        if self.heal.is_busy()
            || self.crop_worker.is_some()
            || self.save_worker.is_some()
            || self.preview_worker.is_some()
        {
            return;
        }
        let result = self
            .current_image
            .as_mut()
            .and_then(Arc::get_mut)
            .map(|image| self.heal.history.undo_patch(image));
        match result {
            Some(Ok(Some(patch))) => {
                self.heal.refresh = None;
                self.update_rendered_patch(&patch);
                self.show_toast("Undid spot heal");
            }
            Some(Err(error)) => self.show_toast(format!("Could not undo edit: {error}")),
            Some(Ok(None)) | None => {}
        }
    }

    fn redo_edit(&mut self) {
        if self.heal.is_busy()
            || self.crop_worker.is_some()
            || self.save_worker.is_some()
            || self.preview_worker.is_some()
        {
            return;
        }
        let result = self
            .current_image
            .as_mut()
            .and_then(Arc::get_mut)
            .map(|image| self.heal.history.redo_patch(image));
        match result {
            Some(Ok(Some(patch))) => {
                self.heal.refresh = None;
                self.update_rendered_patch(&patch);
                self.show_toast("Redid spot heal");
            }
            Some(Err(error)) => self.show_toast(format!("Could not redo edit: {error}")),
            Some(Ok(None)) | None => {}
        }
    }

    fn update_rendered_patch(&mut self, patch: &crate::heal::ImagePatch) {
        let updated = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.update_image_patch(patch));
        if !updated
            && let (Some(image), Some(renderer)) =
                (self.current_image.as_ref(), self.renderer.as_mut())
            && let Err(error) = renderer.set_image(image, None)
        {
            log::error!("failed to restore complete image texture: {error}");
        }
    }

    fn adjust_crop_from_keyboard(&mut self, horizontal: f32, vertical: f32) {
        let Some(image_size) = self.renderer.as_ref().and_then(Renderer::image_size) else {
            return;
        };
        let precision = if self.modifiers.control_key() {
            0.0025
        } else {
            0.01
        };
        let ratio = crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
        let current = self
            .transform
            .crop_rect
            .unwrap_or_else(|| default_crop_rect(image_size, ratio));
        let resize = self.modifiers.shift_key();
        let matrix = crate::view::uv_transform(
            self.transform.rotation_steps,
            self.transform.flip_h,
            self.transform.flip_v,
        );
        let (horizontal, vertical) = crop_keyboard_delta(horizontal, vertical, matrix, resize);
        self.transform.crop_rect = Some(adjust_crop_rect(
            current,
            image_size,
            ratio,
            horizontal * precision,
            vertical * precision,
            resize,
        ));
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
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
        let mut place = crate::view::fit_to_viewport(
            (win_size.width, win_size.height),
            image_size,
            rotated90,
            self.viewport_insets(),
        );
        place.scale[0] *= self.transform.zoom;
        place.scale[1] *= self.transform.zoom;
        place.offset[0] += self.transform.offset_x;
        place.offset[1] += self.transform.offset_y;

        let matrix = crate::view::uv_transform(
            self.transform.rotation_steps,
            self.transform.flip_h,
            self.transform.flip_v,
        );
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

    fn heal_overlay_geometry(&self, scale_factor: f64) -> (Vec<[f32; 2]>, Option<[f32; 2]>, f32) {
        if !self.heal.active || !scale_factor.is_finite() || scale_factor <= 0.0 {
            return (Vec::new(), None, 0.0);
        }
        let Some(image) = self.current_image.as_ref() else {
            return (Vec::new(), None, 0.0);
        };
        let scale = scale_factor as f32;
        let project = |point: crate::heal::StrokePoint| {
            let u = point.x / image.width as f32;
            let v = point.y / image.height as f32;
            self.uv_to_screen(u, v).map(|(x, y)| [x / scale, y / scale])
        };
        let stroke: Vec<[f32; 2]> = self
            .heal
            .stroke
            .iter()
            .filter_map(|point| project(*point))
            .collect();
        let cursor_point = self.heal_point_at(self.cursor_pos);
        let cursor = cursor_point.and_then(project);
        let anchor = cursor_point
            .or_else(|| self.heal.stroke.last().copied())
            .unwrap_or(crate::heal::StrokePoint {
                x: image.width as f32 * 0.5,
                y: image.height as f32 * 0.5,
            });
        let center = project(anchor);
        let edge = project(crate::heal::StrokePoint {
            x: anchor.x + self.heal.brush_radius as f32,
            y: anchor.y,
        });
        let radius = center.zip(edge).map_or(0.0, |(center, edge)| {
            (edge[0] - center[0]).hypot(edge[1] - center[1])
        });
        (stroke, cursor, radius)
    }

    fn update_cursor_icon(&self) {
        if let Some(renderer) = self.renderer.as_ref() {
            let cursor = if self.space_held {
                if self.mouse_left_down {
                    winit::window::CursorIcon::Grabbing
                } else {
                    winit::window::CursorIcon::Grab
                }
            } else if self.transform.is_cropping || self.heal.active {
                winit::window::CursorIcon::Crosshair
            } else {
                winit::window::CursorIcon::Default
            };
            renderer.window().set_cursor(cursor);
        }
    }

    fn zoom_at_cursor(&mut self, factor: f32) {
        self.zoom_at_screen_position(factor, self.cursor_pos);
    }

    fn zoom_at_screen_position(&mut self, factor: f32, screen_position: (f64, f64)) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let win = renderer.window().inner_size();
        let Some(image_size) = renderer.image_size() else {
            return;
        };
        let Some(ndc) = crate::view::cursor_to_ndc(screen_position, (win.width, win.height)) else {
            return;
        };
        let old = self.transform.zoom;
        let new_zoom = (old * factor).clamp(0.05, 64.0);
        let applied = new_zoom / old;
        if (applied - 1.0).abs() < 1e-6 {
            return;
        }
        let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
        let base = crate::view::fit_to_viewport(
            (win.width, win.height),
            image_size,
            rotated90,
            self.viewport_insets(),
        );
        let total_offset = [
            base.offset[0] + self.transform.offset_x,
            base.offset[1] + self.transform.offset_y,
        ];
        let next_total = crate::view::pan_after_zoom_at_cursor(total_offset, ndc, applied);
        self.transform.zoom = new_zoom;
        self.transform.offset_x = next_total[0] - base.offset[0];
        self.transform.offset_y = next_total[1] - base.offset[1];
        if let Some(r) = self.renderer.as_mut() {
            r.window().request_redraw();
        }
    }

    fn zoom_at_viewport_center(&mut self, factor: f32) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let size = renderer.window().inner_size();
        let Some(viewport) =
            crate::view::safe_viewport_rect((size.width, size.height), self.viewport_insets())
        else {
            return;
        };
        let center = (
            f64::from(viewport.x) + f64::from(viewport.width) * 0.5,
            f64::from(viewport.y) + f64::from(viewport.height) * 0.5,
        );
        self.zoom_at_screen_position(factor, center);
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
        filmstrip_range(playlist.index, playlist.files.len())
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
        let paths = self.visible_filmstrip_paths();
        if paths.is_empty() {
            return;
        }
        let visible = paths.iter().cloned().collect::<HashSet<_>>();
        self.thumb_textures.retain(|path, _| visible.contains(path));
        for path in paths {
            if self.thumb_textures.contains_key(&path) || self.thumbs_in_flight.contains(&path) {
                continue;
            }
            let job_path = path.clone();
            let tx = self.thumb_result_tx.clone();
            let event_proxy = self.event_proxy.clone();
            let scheduled = crate::decode::schedule_background_decode(move || {
                let result = match thumbs::generate_thumb(&job_path) {
                    Ok(thumb) => Ok(thumb),
                    Err(err) => Err((job_path, err)),
                };
                let _ = tx.send(result);
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
            if scheduled {
                self.thumbs_in_flight.insert(path);
            }
        }
    }

    fn visible_filmstrip_paths(&self) -> Vec<PathBuf> {
        let Some(playlist) = &self.playlist else {
            return Vec::new();
        };
        playlist.files[filmstrip_range(playlist.index, playlist.files.len())].to_vec()
    }

    fn poll_thumbnails(&mut self) {
        let visible = if self.show_filmstrip_panel && self.filmstrip_panel_open {
            self.visible_filmstrip_paths()
                .into_iter()
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        let mut got = false;
        while let Ok(msg) = self.thumb_rx.try_recv() {
            got = true;
            match msg {
                Ok(thumb) => {
                    self.thumbs_in_flight.remove(&thumb.path);
                    if visible.contains(&thumb.path)
                        && let Some(renderer) = &self.renderer
                    {
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
            if self.show_filmstrip_panel && self.filmstrip_panel_open {
                self.request_thumbs_for_filmstrip();
            }
        }
    }

    fn fit_to_view(&mut self) {
        self.transform.zoom = 1.0;
        self.transform.offset_x = 0.0;
        self.transform.offset_y = 0.0;
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn set_actual_size(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let win = renderer.window().inner_size();
        let Some(image) = renderer.image_size() else {
            return;
        };
        let rotated90 = self.transform.rotation_steps.rem_euclid(2) != 0;
        let fit_scale = crate::view::fit_pixel_scale(
            (win.width, win.height),
            image,
            rotated90,
            self.viewport_insets(),
        );
        let actual_zoom = if fit_scale > 0.0 {
            1.0 / fit_scale
        } else {
            1.0
        };
        self.transform.zoom = actual_zoom;
        self.transform.offset_x = 0.0;
        self.transform.offset_y = 0.0;
        renderer.window().request_redraw();
    }

    fn toggle_fit_actual(&mut self) {
        if (self.transform.zoom - 1.0).abs() < 0.05 {
            self.set_actual_size();
        } else {
            self.fit_to_view();
        }
    }

    fn permanent_delete_current(&mut self) {
        if self.crop_worker.is_some() || self.save_worker.is_some() || self.preview_worker.is_some()
        {
            return;
        }
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
        self.prepare_for_image_load();

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
        let event_proxy = self.event_proxy.clone();
        let current_generation = Arc::clone(&self.image_load_generation);
        let scheduled = crate::decode::schedule_foreground_decode(move || {
            let res = match DecodedImage::load_if_current(&path, &current_generation, generation) {
                Ok(Some(image)) => Ok(image),
                Ok(None) => return,
                Err(error) => Err(error.to_string()),
            };
            let _ = tx.send((path, res));
            let _ = event_proxy.send_event(UserEvent::Wake);
        });
        if let Err(error) = scheduled {
            self.image_loader_rx = None;
            log::error!("failed to queue foreground decode");
            let message = format!("Could not start image decode: {error}");
            self.load_error = Some(message.clone());
            self.show_toast(message);
        }
    }

    fn save_as(&mut self) {
        if self.preview_worker.is_some() {
            self.show_toast("Wait for the image preview to finish before saving");
            return;
        }
        if self.heal.is_busy() || self.heal.painting {
            self.show_toast("Wait for spot heal to finish before saving");
            return;
        }
        if self.crop_worker.is_some() {
            self.show_toast("Wait for the crop to finish before saving");
            return;
        }
        if self.save_worker.is_some() {
            self.show_toast("A copy is already being saved");
            return;
        }
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        let Some(image) = self.current_image.as_ref().map(Arc::clone) else {
            return;
        };
        let pixel_transform = crate::edit::PixelTransform::new(
            self.transform.rotation_steps,
            self.transform.flip_h,
            self.transform.flip_v,
        );
        let default_name = path.with_extension("jpg");
        let Some(file_name) = default_name.file_name() else {
            return;
        };
        let Some(save_path) = rfd::FileDialog::new()
            .set_file_name(file_name.to_string_lossy())
            .add_filter("JPEG", &["jpg", "jpeg"])
            .add_filter("PNG", &["png"])
            .add_filter("WebP", &["webp"])
            .add_filter("BMP", &["bmp"])
            .save_file()
        else {
            return;
        };
        let retain_exif = self.retain_exif;
        let options = if retain_exif {
            crate::edit::SaveOptions::retain_exif()
        } else {
            crate::edit::SaveOptions::strip()
        };
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let spawn = std::thread::Builder::new()
            .name("viewr-save".into())
            .spawn(move || {
                let result = (|| {
                    let transformed = (!pixel_transform.is_identity())
                        .then(|| pixel_transform.apply(image.as_ref()))
                        .transpose()
                        .map_err(|error| error.to_string())?;
                    let export_image = transformed.as_ref().unwrap_or(image.as_ref());
                    crate::edit::save_with_options(export_image, &save_path, Some(&path), options)
                        .map_err(|error| error.to_string())
                })();
                let _ = sender.send(result);
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
        match spawn {
            Ok(_) => {
                self.save_worker = Some(SaveWorker {
                    result_rx: receiver,
                });
                self.show_toast("Saving copy in the background");
            }
            Err(error) => {
                log::error!("failed to start save worker");
                self.show_toast(format!("Could not start save: {error}"));
            }
        }
    }

    fn poll_save_result(&mut self) {
        let result = match self
            .save_worker
            .as_ref()
            .map(|worker| worker.result_rx.try_recv())
        {
            Some(Ok(result)) => result,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.save_worker = None;
                self.show_toast("Save stopped unexpectedly");
                return;
            }
            Some(Err(mpsc::TryRecvError::Empty)) | None => return,
        };
        self.save_worker = None;
        match result {
            Ok(crate::edit::MetadataDisposition::Retained) => {
                self.show_toast("Saved copy · EXIF retained");
            }
            Ok(crate::edit::MetadataDisposition::NotPresent) => {
                self.show_toast("Saved copy · no EXIF found");
            }
            Ok(crate::edit::MetadataDisposition::Stripped) => {
                self.show_toast("Saved copy · metadata stripped");
            }
            Err(error) => {
                log::error!("failed to save image");
                self.show_toast(format!("Save failed: {error}"));
            }
        }
    }

    /// Convert a UV crop rect into one bounded pixel rectangle. Locked ratios
    /// are quantized as whole multiples of their reduced integer components,
    /// so the exported pixel dimensions keep the ratio exactly.
    fn crop_pixel_rect(
        rect: [f32; 4],
        width: u32,
        height: u32,
        ratio: CropRatio,
    ) -> Option<crate::edit::Rect> {
        if width == 0 || height == 0 {
            return None;
        }
        let left = f64::from(rect[0].min(rect[2]).clamp(0.0, 1.0));
        let top = f64::from(rect[1].min(rect[3]).clamp(0.0, 1.0));
        let right = f64::from(rect[0].max(rect[2]).clamp(0.0, 1.0));
        let bottom = f64::from(rect[1].max(rect[3]).clamp(0.0, 1.0));

        let Some((ratio_width, ratio_height)) = crop_integer_ratio((width, height), ratio) else {
            let x = nonnegative_floor_u32(left * f64::from(width)).min(width);
            let y = nonnegative_floor_u32(top * f64::from(height)).min(height);
            let right = nonnegative_ceil_u32(right * f64::from(width)).min(width);
            let bottom = nonnegative_ceil_u32(bottom * f64::from(height)).min(height);
            let crop_width = right.saturating_sub(x);
            let crop_height = bottom.saturating_sub(y);
            return (crop_width != 0 && crop_height != 0).then_some(crate::edit::Rect {
                x,
                y,
                width: crop_width,
                height: crop_height,
            });
        };

        let maximum_scale = (width / ratio_width).min(height / ratio_height);
        if maximum_scale == 0 {
            return None;
        }
        let selected_width = (right - left) * f64::from(width);
        let selected_height = (bottom - top) * f64::from(height);
        let desired_scale = (selected_width / f64::from(ratio_width))
            .min(selected_height / f64::from(ratio_height))
            .round()
            .clamp(1.0, f64::from(maximum_scale));
        let scale = nonnegative_floor_u32(desired_scale).clamp(1, maximum_scale);
        let crop_width = ratio_width.checked_mul(scale)?;
        let crop_height = ratio_height.checked_mul(scale)?;
        let center_x = (left + right) * 0.5 * f64::from(width);
        let center_y = (top + bottom) * 0.5 * f64::from(height);
        let x = centered_crop_origin(center_x, crop_width, width);
        let y = centered_crop_origin(center_y, crop_height, height);
        Some(crate::edit::Rect {
            x,
            y,
            width: crop_width,
            height: crop_height,
        })
    }

    fn apply_crop_rect(&mut self) {
        let Some(source_path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        if self.crop_worker.is_some() {
            self.show_toast("A crop is already being applied");
            return;
        }
        if self.preview_worker.is_some() {
            self.show_toast("Wait for the image preview to finish before cropping");
            return;
        }
        if self.save_worker.is_some() {
            self.show_toast("Wait for the current save to finish before cropping");
            return;
        }
        if self.heal.is_busy() {
            self.show_toast("Wait for spot heal to finish before cropping");
            return;
        }
        let Some(rect) = self.transform.crop_rect else {
            return;
        };
        let Some(image) = self.current_image.as_ref().map(Arc::clone) else {
            return;
        };
        let ratio = crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
        let Some(pixel_rect) = Self::crop_pixel_rect(rect, image.width, image.height, ratio) else {
            self.show_toast("The selected ratio is too large for this image");
            return;
        };

        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let spawn = std::thread::Builder::new()
            .name("viewr-crop".into())
            .spawn(move || {
                let cropped = crate::edit::crop(image.as_ref(), pixel_rect)
                    .map_err(|error| error.to_string());
                let _ = sender.send(cropped);
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
        if let Err(error) = spawn {
            self.show_toast(format!("Could not start crop: {error}"));
            return;
        }

        self.discard_animation_for_pixel_edit();
        self.transform.zoom = 1.0;
        self.transform.offset_x = 0.0;
        self.transform.offset_y = 0.0;
        self.transform.is_panning = false;
        self.transform.last_cursor = None;
        self.transform.crop_rect = None;
        self.transform.is_cropping = false;
        self.transform.crop_start = None;
        self.crop_worker = Some(CropWorker {
            source_path,
            result_rx: receiver,
        });
        self.show_toast("Applying crop in the background");
        self.request_redraw();
    }

    fn poll_crop_result(&mut self) {
        let polled = self
            .crop_worker
            .as_ref()
            .map(|worker| (worker.source_path.clone(), worker.result_rx.try_recv()));
        let (source_path, cropped) = match polled {
            Some((source_path, Ok(Ok(cropped)))) => (source_path, cropped),
            Some((_, Ok(Err(error)))) => {
                self.crop_worker = None;
                self.show_toast(format!("Crop failed: {error}"));
                return;
            }
            Some((_, Err(mpsc::TryRecvError::Disconnected))) => {
                self.crop_worker = None;
                self.show_toast("Crop stopped unexpectedly");
                return;
            }
            Some((_, Err(mpsc::TryRecvError::Empty))) | None => return,
        };
        self.crop_worker = None;

        if self.image_path.as_ref() != Some(&source_path)
            || self.loaded_image_path.as_ref() != Some(&source_path)
        {
            return;
        }
        self.present_image(&source_path, Arc::new(cropped), PresentationKind::Cropped);
        self.request_redraw();
    }

    fn performance_probe_has_presented_current(&self) -> bool {
        let Some(probe) = self.performance_probe.as_ref() else {
            return false;
        };
        let Some(current_path) = self.current_loaded_path() else {
            return false;
        };
        if probe.last_presented_path.as_deref() != Some(current_path)
            || probe.navigation_target.is_some()
            || self.image_loader_rx.is_some()
            || self.preview_worker.is_some()
            || self.auxiliary_loader_rx.is_some()
            || self.crop_worker.is_some()
            || self.scanner_rx.is_some()
        {
            return false;
        }
        true
    }

    fn performance_probe_is_settled(&self) -> bool {
        if !self.performance_probe_has_presented_current()
            || !self.prefetch_in_flight.is_empty()
            || !self.thumbs_in_flight.is_empty()
            || self.auxiliary_loader_rx.is_some()
        {
            return false;
        }
        // A scheduled egui repaint is exactly what the idle observation window
        // is meant to measure. Treating it as background work can starve the
        // probe when hover or accessibility state keeps requesting frames.
        self.visible_filmstrip_paths()
            .iter()
            .all(|path| self.thumb_textures.contains_key(path))
    }

    fn fail_performance_probe(&mut self, event_loop: &ActiveEventLoop, message: String) {
        if let Some(probe) = self.performance_probe.as_mut() {
            probe.outcome = Some(Err(message));
        }
        event_loop.exit();
    }

    fn performance_probe_timeout_message(&self) -> String {
        let visible = self.visible_filmstrip_paths();
        let ready_thumbnails = visible
            .iter()
            .filter(|path| self.thumb_textures.contains_key(*path))
            .count();
        let remaining_navigation = self
            .performance_probe
            .as_ref()
            .and_then(|probe| probe.navigation_targets.as_ref())
            .map_or(0, VecDeque::len);
        let presented_current = self.current_loaded_path().is_some_and(|path| {
            self.performance_probe
                .as_ref()
                .and_then(|probe| probe.last_presented_path.as_deref())
                == Some(path)
        });
        format!(
            concat!(
                "probe exceeded its {} second deadline ",
                "(scan={}, image={}, auxiliary={}, navigation={}, remaining_navigation={}, ",
                "presented_current={}, idle_started={}, prefetch={}, thumbnails={}, ",
                "ready_thumbnails={}/{})"
            ),
            PERFORMANCE_PROBE_TIMEOUT.as_secs(),
            self.scanner_rx.is_some(),
            self.image_loader_rx.is_some() || self.preview_worker.is_some(),
            self.auxiliary_loader_rx.is_some(),
            self.performance_probe
                .as_ref()
                .is_some_and(|probe| probe.navigation_target.is_some()),
            remaining_navigation,
            presented_current,
            self.performance_probe
                .as_ref()
                .is_some_and(|probe| probe.idle_until.is_some()),
            self.prefetch_in_flight.len(),
            self.thumbs_in_flight.len(),
            ready_thumbnails,
            visible.len(),
        )
    }

    fn begin_performance_probe_idle(&mut self, event_loop: &ActiveEventLoop, now: Instant) {
        let idle_until = now + PERFORMANCE_IDLE_OBSERVATION;
        self.performance_probe.as_mut().unwrap().idle_until = Some(idle_until);
        if let Err(error) = schedule_performance_wake(
            self.event_proxy.clone(),
            "viewr-performance-idle",
            idle_until,
        ) {
            self.fail_performance_probe(event_loop, error);
        }
    }

    fn advance_performance_probe(&mut self, event_loop: &ActiveEventLoop) {
        let Some(deadline) = self.performance_probe.as_ref().map(|probe| probe.deadline) else {
            return;
        };
        if Instant::now() >= deadline {
            let message = self.performance_probe_timeout_message();
            self.fail_performance_probe(event_loop, message);
            return;
        }
        if !self.performance_probe_has_presented_current() {
            return;
        }
        let (current_index, playlist_len) = self
            .playlist
            .as_ref()
            .map_or((0, 0), |playlist| (playlist.index, playlist.files.len()));
        let probe = self.performance_probe.as_mut().unwrap();
        if probe.navigation_targets.is_none() {
            probe.navigation_targets =
                Some(performance_navigation_targets(current_index, playlist_len));
        }
        let next_index = probe
            .navigation_targets
            .as_mut()
            .and_then(VecDeque::pop_front);
        if let Some(next_index) = next_index {
            let next_path = self.playlist.as_ref().unwrap().files[next_index].clone();
            let probe = self.performance_probe.as_mut().unwrap();
            probe.navigation_started = Some(Instant::now());
            probe.navigation_target = Some(next_path);
            self.go_to_index(next_index);
            if let Some(renderer) = self.renderer.as_ref() {
                renderer.window().request_redraw();
            }
            return;
        }

        if !self.performance_probe_is_settled() {
            if let Some(probe) = self.performance_probe.as_mut()
                && probe.idle_until.is_some()
            {
                probe.reset_idle_observation();
            }
            return;
        }

        let resident_bytes = match crate::performance::peak_resident_bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail_performance_probe(
                    event_loop,
                    format!("could not measure resident memory: {error}"),
                );
                return;
            }
        };
        let probe = self.performance_probe.as_mut().unwrap();
        probe.peak_resident_bytes = probe.peak_resident_bytes.max(resident_bytes);

        let now = Instant::now();
        let probe = self.performance_probe.as_mut().unwrap();
        if let Some(idle_until) = probe.idle_until {
            if now < idle_until {
                return;
            }
        } else {
            self.begin_performance_probe_idle(event_loop, now);
            return;
        }

        let probe = self.performance_probe.as_ref().unwrap();
        let Some(window_ready) = probe.window_ready else {
            self.fail_performance_probe(event_loop, "probe never observed a visible window".into());
            return;
        };
        let Some(first_pixel) = probe.first_pixel else {
            self.fail_performance_probe(event_loop, "probe never presented an image frame".into());
            return;
        };
        let report = crate::performance::PerformanceReport {
            window_ready_us: crate::performance::duration_us(window_ready),
            first_pixel_us: crate::performance::duration_us(first_pixel),
            max_navigation_us: crate::performance::duration_us(probe.max_navigation),
            idle_redraws: probe.idle_redraws,
            peak_resident_bytes: probe.peak_resident_bytes,
            playlist_entries: playlist_len,
            decoded_cache_entries: self.prefetch.len(),
            decoded_cache_bytes: u64::try_from(self.prefetch.bytes()).unwrap_or(u64::MAX),
            thumbnail_texture_entries: self.thumb_textures.len(),
        };
        let outcome = validate_performance_report(report);
        self.performance_probe.as_mut().unwrap().outcome = Some(outcome);
        event_loop.exit();
    }
}

const DEFAULT_CROP_MARGIN: f32 = 0.1;
const MINIMUM_CROP_SPAN: f32 = 0.02;

/// Convert a ratio chosen in visible/output orientation to the decoded source
/// axes used by crop geometry. A quarter turn swaps width and height.
pub(crate) fn crop_ratio_for_source(ratio: CropRatio, rotation_steps: i32) -> CropRatio {
    if rotation_steps.rem_euclid(2) == 0 {
        return ratio;
    }
    match ratio {
        CropRatio::Fixed { width, height } => CropRatio::fixed(height, width),
        CropRatio::Free | CropRatio::Original => ratio,
    }
}

/// Quantize a normalized crop selection through the exact exporter path.
/// Chrome and accessibility consumers use this seam so announced pixel bounds
/// cannot drift from the full-resolution crop that will be written.
pub(crate) fn quantized_crop_pixel_rect(
    rect: [f32; 4],
    width: u32,
    height: u32,
    ratio: CropRatio,
) -> Option<crate::edit::Rect> {
    App::crop_pixel_rect(rect, width, height, ratio)
}

fn crop_integer_ratio(image_size: (u32, u32), ratio: CropRatio) -> Option<(u32, u32)> {
    let (width, height) = match ratio {
        CropRatio::Original => image_size,
        CropRatio::Fixed { width, height } if width != 0 && height != 0 => {
            (u32::from(width), u32::from(height))
        }
        CropRatio::Free | CropRatio::Fixed { .. } => return None,
    };
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(width, height);
    Some((width / divisor, height / divisor))
}

const fn greatest_common_divisor(mut left: u32, mut right: u32) -> u32 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn reduced_crop_ratio(width: u32, height: u32) -> Option<(u16, u16)> {
    if width == 0 || height == 0 {
        return None;
    }
    let divisor = greatest_common_divisor(width, height);
    let width = u16::try_from(width / divisor).ok()?;
    let height = u16::try_from(height / divisor).ok()?;
    Some((width, height))
}

fn crop_pixel_aspect(image_size: (u32, u32), ratio: CropRatio) -> Option<f32> {
    let (image_width, image_height) = image_size;
    if image_width == 0 || image_height == 0 {
        return None;
    }
    match ratio {
        CropRatio::Original => Some(image_width as f32 / image_height as f32),
        CropRatio::Fixed { width, height } if width != 0 && height != 0 => {
            Some(f32::from(width) / f32::from(height))
        }
        CropRatio::Free | CropRatio::Fixed { .. } => None,
    }
}

fn crop_uv_aspect(image_size: (u32, u32), ratio: CropRatio) -> Option<f32> {
    let (image_width, image_height) = image_size;
    let pixel_aspect = crop_pixel_aspect(image_size, ratio)?;
    Some(pixel_aspect * image_height as f32 / image_width as f32)
}

fn default_crop_rect(image_size: (u32, u32), ratio: CropRatio) -> [f32; 4] {
    fit_crop_rect_to_ratio(
        [
            DEFAULT_CROP_MARGIN,
            DEFAULT_CROP_MARGIN,
            1.0 - DEFAULT_CROP_MARGIN,
            1.0 - DEFAULT_CROP_MARGIN,
        ],
        image_size,
        ratio,
    )
}

fn fit_crop_rect_to_ratio(bounds: [f32; 4], image_size: (u32, u32), ratio: CropRatio) -> [f32; 4] {
    let left = bounds[0].min(bounds[2]).clamp(0.0, 1.0);
    let top = bounds[1].min(bounds[3]).clamp(0.0, 1.0);
    let right = bounds[0].max(bounds[2]).clamp(left, 1.0);
    let bottom = bounds[1].max(bounds[3]).clamp(top, 1.0);
    let Some(aspect) = crop_uv_aspect(image_size, ratio) else {
        return [left, top, right, bottom];
    };
    let available_width = right - left;
    let available_height = bottom - top;
    if available_width <= 0.0 || available_height <= 0.0 {
        return [left, top, right, bottom];
    }

    let (width, height) = if available_width / available_height > aspect {
        (available_height * aspect, available_height)
    } else {
        (available_width, available_width / aspect)
    };
    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    [
        center_x - width * 0.5,
        center_y - height * 0.5,
        center_x + width * 0.5,
        center_y + height * 0.5,
    ]
}

fn crop_handle_from_uv(rect: [f32; 4], point: (f32, f32)) -> CropHandle {
    fn nearest_zone(value: f32, start: f32, end: f32) -> i8 {
        let center = (start + end) * 0.5;
        let candidates = [
            ((value - start).abs(), -1),
            ((value - center).abs(), 0),
            ((value - end).abs(), 1),
        ];
        candidates
            .into_iter()
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map_or(0, |candidate| candidate.1)
    }

    let left = rect[0].min(rect[2]);
    let top = rect[1].min(rect[3]);
    let right = rect[0].max(rect[2]);
    let bottom = rect[1].max(rect[3]);
    match (
        nearest_zone(point.0, left, right),
        nearest_zone(point.1, top, bottom),
    ) {
        (-1, -1) => CropHandle::TopLeft,
        (0, -1) => CropHandle::Top,
        (1, -1) => CropHandle::TopRight,
        (1, 0) => CropHandle::Right,
        (1, 1) => CropHandle::BottomRight,
        (0, 1) => CropHandle::Bottom,
        (-1, 1) => CropHandle::BottomLeft,
        (-1, 0) => CropHandle::Left,
        // A handle center never maps to the middle, but choosing the nearest
        // horizontal edge is a deterministic fallback for malformed geometry.
        (0, 0) => {
            if (point.0 - left).abs() <= (right - point.0).abs() {
                CropHandle::Left
            } else {
                CropHandle::Right
            }
        }
        _ => CropHandle::Right,
    }
}

fn resize_crop_rect_from_pointer(
    rect: [f32; 4],
    image_size: (u32, u32),
    ratio: CropRatio,
    handle: CropHandle,
    pointer: (f32, f32),
) -> [f32; 4] {
    let normalized = [
        rect[0].min(rect[2]).clamp(0.0, 1.0),
        rect[1].min(rect[3]).clamp(0.0, 1.0),
        rect[0].max(rect[2]).clamp(0.0, 1.0),
        rect[1].max(rect[3]).clamp(0.0, 1.0),
    ];
    let pointer = (pointer.0.clamp(0.0, 1.0), pointer.1.clamp(0.0, 1.0));
    crop_uv_aspect(image_size, ratio).map_or_else(
        || resize_free_crop_rect(normalized, handle, pointer),
        |aspect| resize_locked_crop_rect(normalized, handle, pointer, aspect),
    )
}

fn resize_free_crop_rect(
    [left, top, right, bottom]: [f32; 4],
    handle: CropHandle,
    pointer: (f32, f32),
) -> [f32; 4] {
    let next_left = pointer.0.min(right - MINIMUM_CROP_SPAN);
    let next_right = pointer.0.max(left + MINIMUM_CROP_SPAN);
    let next_top = pointer.1.min(bottom - MINIMUM_CROP_SPAN);
    let next_bottom = pointer.1.max(top + MINIMUM_CROP_SPAN);
    match handle {
        CropHandle::TopLeft => [next_left, next_top, right, bottom],
        CropHandle::Top => [left, next_top, right, bottom],
        CropHandle::TopRight => [left, next_top, next_right, bottom],
        CropHandle::Right => [left, top, next_right, bottom],
        CropHandle::BottomRight => [left, top, next_right, next_bottom],
        CropHandle::Bottom => [left, top, right, next_bottom],
        CropHandle::BottomLeft => [next_left, top, right, next_bottom],
        CropHandle::Left => [next_left, top, right, bottom],
    }
}

fn resize_locked_crop_rect(
    rect: [f32; 4],
    handle: CropHandle,
    pointer: (f32, f32),
    aspect: f32,
) -> [f32; 4] {
    match handle {
        CropHandle::TopLeft => {
            locked_corner_crop((rect[2], rect[3]), pointer, (-1.0, -1.0), aspect)
        }
        CropHandle::TopRight => {
            locked_corner_crop((rect[0], rect[3]), pointer, (1.0, -1.0), aspect)
        }
        CropHandle::BottomRight => {
            locked_corner_crop((rect[0], rect[1]), pointer, (1.0, 1.0), aspect)
        }
        CropHandle::BottomLeft => {
            locked_corner_crop((rect[2], rect[1]), pointer, (-1.0, 1.0), aspect)
        }
        CropHandle::Left | CropHandle::Right => {
            locked_horizontal_edge_crop(rect, handle, pointer.0, aspect)
        }
        CropHandle::Top | CropHandle::Bottom => {
            locked_vertical_edge_crop(rect, handle, pointer.1, aspect)
        }
    }
}

fn fit_locked_extent(
    desired_width: f32,
    desired_height: f32,
    maximum_width: f32,
    maximum_height: f32,
    aspect: f32,
) -> (f32, f32) {
    let mut width = desired_width
        .max(desired_height * aspect)
        .max(MINIMUM_CROP_SPAN)
        .max(MINIMUM_CROP_SPAN * aspect);
    let mut height = width / aspect;
    let scale = (maximum_width / width)
        .min(maximum_height / height)
        .clamp(0.0, 1.0);
    width *= scale;
    height *= scale;
    (width, height)
}

fn locked_corner_crop(
    anchor: (f32, f32),
    pointer: (f32, f32),
    direction: (f32, f32),
    aspect: f32,
) -> [f32; 4] {
    let maximum_width = if direction.0 < 0.0 {
        anchor.0
    } else {
        1.0 - anchor.0
    };
    let maximum_height = if direction.1 < 0.0 {
        anchor.1
    } else {
        1.0 - anchor.1
    };
    let (width, height) = fit_locked_extent(
        (pointer.0 - anchor.0).abs(),
        (pointer.1 - anchor.1).abs(),
        maximum_width,
        maximum_height,
        aspect,
    );
    let (left, right) = if direction.0 < 0.0 {
        (anchor.0 - width, anchor.0)
    } else {
        (anchor.0, anchor.0 + width)
    };
    let (top, bottom) = if direction.1 < 0.0 {
        (anchor.1 - height, anchor.1)
    } else {
        (anchor.1, anchor.1 + height)
    };
    [left, top, right, bottom]
}

fn locked_horizontal_edge_crop(
    [left, top, right, bottom]: [f32; 4],
    handle: CropHandle,
    pointer_x: f32,
    aspect: f32,
) -> [f32; 4] {
    let center_y = (top + bottom) * 0.5;
    let (anchor_x, direction) = if handle == CropHandle::Left {
        (right, -1.0)
    } else {
        (left, 1.0)
    };
    let maximum_width = if direction < 0.0 {
        anchor_x
    } else {
        1.0 - anchor_x
    };
    let maximum_height = 2.0 * center_y.min(1.0 - center_y);
    let (width, height) = fit_locked_extent(
        (pointer_x - anchor_x).abs(),
        0.0,
        maximum_width,
        maximum_height,
        aspect,
    );
    let (left, right) = if direction < 0.0 {
        (anchor_x - width, anchor_x)
    } else {
        (anchor_x, anchor_x + width)
    };
    [
        left,
        center_y - height * 0.5,
        right,
        center_y + height * 0.5,
    ]
}

fn locked_vertical_edge_crop(
    [left, top, right, bottom]: [f32; 4],
    handle: CropHandle,
    pointer_y: f32,
    aspect: f32,
) -> [f32; 4] {
    let center_x = (left + right) * 0.5;
    let (anchor_y, direction) = if handle == CropHandle::Top {
        (bottom, -1.0)
    } else {
        (top, 1.0)
    };
    let maximum_width = 2.0 * center_x.min(1.0 - center_x);
    let maximum_height = if direction < 0.0 {
        anchor_y
    } else {
        1.0 - anchor_y
    };
    let (width, height) = fit_locked_extent(
        0.0,
        (pointer_y - anchor_y).abs(),
        maximum_width,
        maximum_height,
        aspect,
    );
    let (top, bottom) = if direction < 0.0 {
        (anchor_y - height, anchor_y)
    } else {
        (anchor_y, anchor_y + height)
    };
    [center_x - width * 0.5, top, center_x + width * 0.5, bottom]
}

fn adjust_crop_rect(
    rect: [f32; 4],
    image_size: (u32, u32),
    ratio: CropRatio,
    horizontal: f32,
    vertical: f32,
    resize: bool,
) -> [f32; 4] {
    let left = rect[0].min(rect[2]).clamp(0.0, 1.0);
    let top = rect[1].min(rect[3]).clamp(0.0, 1.0);
    let right = rect[0].max(rect[2]).clamp(left, 1.0);
    let bottom = rect[1].max(rect[3]).clamp(top, 1.0);
    let width = (right - left).clamp(MINIMUM_CROP_SPAN, 1.0);
    let height = (bottom - top).clamp(MINIMUM_CROP_SPAN, 1.0);

    if !resize {
        let next_left = (left + horizontal).clamp(0.0, 1.0 - width);
        let next_top = (top + vertical).clamp(0.0, 1.0 - height);
        return [next_left, next_top, next_left + width, next_top + height];
    }

    let mut next_width = (width + horizontal).clamp(MINIMUM_CROP_SPAN, 1.0);
    let mut next_height = (height + vertical).clamp(MINIMUM_CROP_SPAN, 1.0);
    if let Some(aspect) = crop_uv_aspect(image_size, ratio) {
        if horizontal.abs() >= vertical.abs() {
            next_height = next_width / aspect;
        } else {
            next_width = next_height * aspect;
        }
        if next_width < MINIMUM_CROP_SPAN {
            next_width = MINIMUM_CROP_SPAN;
            next_height = next_width / aspect;
        }
        if next_height < MINIMUM_CROP_SPAN {
            next_height = MINIMUM_CROP_SPAN;
            next_width = next_height * aspect;
        }
        let fit_scale = (1.0 / next_width).min(1.0 / next_height).min(1.0);
        next_width *= fit_scale;
        next_height *= fit_scale;
    }

    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    let next_left = (center_x - next_width * 0.5).clamp(0.0, 1.0 - next_width);
    let next_top = (center_y - next_height * 0.5).clamp(0.0, 1.0 - next_height);
    [
        next_left,
        next_top,
        next_left + next_width,
        next_top + next_height,
    ]
}

fn crop_keyboard_delta(
    horizontal: f32,
    vertical: f32,
    uv_matrix: [f32; 4],
    resize: bool,
) -> (f32, f32) {
    if !resize {
        return (
            uv_matrix[0] * horizontal + uv_matrix[2] * vertical,
            uv_matrix[1] * horizontal + uv_matrix[3] * vertical,
        );
    }

    // Resize is centered: right/down always grow and left/up always shrink.
    // Rotation selects the source axis, while flips must not reverse growth.
    if horizontal.abs() > f32::EPSILON {
        if uv_matrix[0].abs() >= uv_matrix[1].abs() {
            (horizontal, 0.0)
        } else {
            (0.0, horizontal)
        }
    } else if uv_matrix[2].abs() >= uv_matrix[3].abs() {
        (vertical, 0.0)
    } else {
        (0.0, vertical)
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
    let chrome_width = f64::from(crate::ui::TOOLS_RAIL_WIDTH);
    let chrome_height = f64::from(crate::ui::TOP_BAR_HEIGHT + crate::ui::FILMSTRIP_RAIL_HEIGHT);
    let maximum_width = available_width * 0.88;
    let maximum_height = available_height * 0.86;
    let fit_scale = ((maximum_width - chrome_width) / image_width)
        .min((maximum_height - chrome_height) / image_height)
        .clamp(0.0, 1.0);
    let minimum_width = 840.0_f64.min(maximum_width);
    let minimum_height = 620.0_f64.min(maximum_height);
    let desired_width = (image_width * fit_scale + chrome_width)
        .max(minimum_width)
        .min(maximum_width);
    let desired_height = (image_height * fit_scale + chrome_height)
        .max(minimum_height)
        .min(maximum_height);
    let size = LogicalSize::new(desired_width, desired_height);
    let _ = renderer.window().request_inner_size(size);
}

fn nonnegative_floor_u32(value: f64) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= f64::from(u32::MAX) {
        return u32::MAX;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value.floor() as u32
    }
}

fn nonnegative_ceil_u32(value: f64) -> u32 {
    nonnegative_floor_u32(value.ceil())
}

fn centered_crop_origin(center: f64, extent: u32, bound: u32) -> u32 {
    let maximum = bound.saturating_sub(extent);
    let origin = (center - f64::from(extent) * 0.5)
        .round()
        .clamp(0.0, f64::from(maximum));
    nonnegative_floor_u32(origin)
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
            .with_inner_size(LogicalSize::new(1000.0, 720.0))
            .with_min_inner_size(LogicalSize::new(640.0, 480.0))
            .with_theme(self.theme_preference.window_theme())
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

        let mode = self.theme_preference.resolve(window.theme());
        let max_base_pixels = if self.performance_probe.is_some() {
            crate::gpu::PERFORMANCE_PROBE_GPU_BASE_PIXELS
        } else {
            crate::gpu::MAX_GPU_BASE_PIXELS
        };
        match pollster::block_on(Renderer::new(window, mode, max_base_pixels)) {
            Ok(renderer) => {
                #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
                let mut renderer = renderer;
                #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
                renderer.init_accessibility(event_loop, self.event_proxy.clone());
                self.renderer = Some(renderer);
                let should_resize = self
                    .loaded_image_path
                    .as_deref()
                    .is_some_and(|path| self.resize_on_load.as_deref() == Some(path));
                if let Some(image) = self.current_image.as_ref() {
                    let image = Arc::clone(image);
                    let path = self.loaded_image_path.clone();
                    if let Some(path) = path
                        && self
                            .renderer
                            .as_ref()
                            .is_some_and(|renderer| renderer.required_preview(&image).is_some())
                    {
                        self.present_image(&path, image, PresentationKind::Loaded);
                    } else if let Some(renderer) = self.renderer.as_mut() {
                        if let Err(error) = renderer.set_image(&image, None) {
                            log::error!("failed to upload initial image: {error}");
                        } else if should_resize {
                            resize_window_to_image(renderer);
                        }
                    }
                }
                if should_resize && self.preview_worker.is_none() {
                    self.resize_on_load = None;
                }
                let _ = self.renderer.as_mut().unwrap().render(None, None, |_| {});
                let window = self.renderer.as_ref().unwrap().window();
                window.set_visible(true);
                window.request_redraw();
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

        let mut egui_consumed = false;
        let mut egui_popup_open = false;
        if let Some(renderer) = &mut self.renderer
            && renderer.window().id() == window_id
        {
            let window = renderer.window.clone();
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            renderer.process_accessibility_window_event(window.as_ref(), &event);
            let response = renderer.egui_state.on_window_event(window.as_ref(), &event);
            // egui reports that RedrawRequested itself wants repainting. The
            // current event already satisfies that request, so scheduling it
            // again here would create a permanent redraw loop.
            if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
                window.request_redraw();
            }
            egui_consumed = response.consumed;
            egui_popup_open = egui::Popup::is_any_open(&renderer.egui_ctx);
        }

        if egui_consumed {
            let application_must_handle = match &event {
                WindowEvent::CloseRequested
                | WindowEvent::DroppedFile(_)
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ThemeChanged(_)
                | WindowEvent::Focused(_)
                | WindowEvent::CursorLeft { .. }
                | WindowEvent::MouseInput {
                    state: winit::event::ElementState::Released,
                    button: winit::event::MouseButton::Left,
                    ..
                } => true,
                WindowEvent::CursorMoved { .. } if self.heal.painting => true,
                WindowEvent::KeyboardInput { event, .. } => {
                    !self.show_about
                        && !egui_popup_open
                        && route_consumed_keyboard_key(
                            &event.logical_key,
                            self.transform.is_cropping,
                            self.heal.active,
                        )
                }
                _ => false,
            };
            if !application_must_handle {
                return;
            }
        }

        match event {
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }
            WindowEvent::Focused(false) => {
                self.mouse_left_down = false;
                self.space_held = false;
                self.space_dragged = false;
                self.transform.is_panning = false;
                self.transform.crop_start = None;
                self.heal.painting = false;
                self.heal.stroke.clear();
                self.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                if self.heal.painting {
                    self.finish_heal_stroke();
                }
                self.mouse_left_down = false;
                self.transform.is_panning = false;
                self.transform.crop_start = None;
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::DroppedFile(path) => {
                self.load_and_scan(path);
            }
            WindowEvent::MouseWheel { delta, .. } => {
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
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == winit::event::ElementState::Pressed;
                if button == winit::event::MouseButton::Right {
                    self.mouse_right_down = pressed;
                    if pressed {
                        self.right_click_start = Some(self.cursor_pos);
                        self.context_menu_pos = None;
                    } else {
                        if let Some(start) = self.right_click_start {
                            let dist =
                                (self.cursor_pos.0 - start.0).hypot(self.cursor_pos.1 - start.1);
                            if dist < 5.0 {
                                self.context_menu_pos =
                                    Some([self.cursor_pos.0 as f32, self.cursor_pos.1 as f32]);
                                if let Some(renderer) = self.renderer.as_mut() {
                                    renderer.window().request_redraw();
                                }
                            }
                        }
                        self.right_click_start = None;
                    }
                } else if button == winit::event::MouseButton::Left {
                    if pressed {
                        self.context_menu_pos = None;
                    }
                    self.mouse_left_down = pressed;
                    self.update_cursor_icon();
                    if !pressed {
                        self.transform.is_panning = false;
                    }
                    if pressed && !self.transform.is_cropping && !self.heal.active {
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
                    if !pressed && self.heal.painting {
                        self.finish_heal_stroke();
                    } else if self.heal.active && !self.space_held {
                        if pressed {
                            self.begin_heal_stroke();
                        }
                    } else if self.transform.is_cropping && !self.space_held {
                        if pressed {
                            if let Some((x, y)) = self.transform.last_cursor {
                                self.transform.crop_start = self.screen_to_uv(x, y);
                                if let Some(renderer) = self.renderer.as_mut() {
                                    renderer.window().request_redraw();
                                }
                            }
                        } else {
                            self.transform.crop_start = None;
                        }
                    } else {
                        self.transform.is_panning = pressed;
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let dx = position.x - self.cursor_pos.0;
                self.cursor_pos = (position.x, position.y);
                if self.mouse_right_down && self.heal.active {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let new_radius = (f64::from(self.heal.brush_radius) + dx * 0.5).clamp(
                        f64::from(crate::heal::MIN_BRUSH_RADIUS),
                        f64::from(crate::heal::MAX_BRUSH_RADIUS),
                    ) as u32;
                    if new_radius != self.heal.brush_radius {
                        self.heal.brush_radius = new_radius;
                        if let Some(renderer) = self.renderer.as_mut() {
                            renderer.window().request_redraw();
                        }
                    }
                } else if self.heal.active
                    && self.heal.painting
                    && self.mouse_left_down
                    && !self.space_held
                {
                    self.continue_heal_stroke();
                    self.request_redraw();
                } else if self.transform.is_cropping
                    && let Some(start) = self.transform.crop_start
                    && let Some(end) = self.screen_to_uv(position.x, position.y)
                {
                    let mut u_min = start.0.min(end.0).clamp(0.0, 1.0);
                    let mut v_min = start.1.min(end.1).clamp(0.0, 1.0);
                    let mut u_max = start.0.max(end.0).clamp(0.0, 1.0);
                    let mut v_max = start.1.max(end.1).clamp(0.0, 1.0);

                    if let Some(renderer) = self.renderer.as_ref()
                        && let Some((img_w, img_h)) = renderer.image_size()
                        && let Some(target_ratio) = crop_pixel_aspect(
                            (img_w, img_h),
                            crop_ratio_for_source(
                                self.transform.crop_ratio,
                                self.transform.rotation_steps,
                            ),
                        )
                    {
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
                    if let Some(renderer) = self.renderer.as_mut() {
                        renderer.window().request_redraw();
                    }
                } else if self.mouse_left_down
                    && (self.transform.is_panning || self.space_held)
                    && let Some((last_x, last_y)) = self.transform.last_cursor
                {
                    if self.space_held {
                        self.space_dragged = true;
                    }
                    let dx = position.x - last_x;
                    let dy = position.y - last_y;
                    if let Some(renderer) = self.renderer.as_mut() {
                        let win_size = renderer.window().inner_size();
                        self.transform.offset_x += (dx as f32) / (win_size.width as f32 / 2.0);
                        self.transform.offset_y -= (dy as f32) / (win_size.height as f32 / 2.0);
                        renderer.window().request_redraw();
                    }
                }
                self.transform.last_cursor = Some((position.x, position.y));

                self.update_cursor_icon();
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        state, logical_key, ..
                    },
                ..
            } => {
                use winit::keyboard::{Key, NamedKey};
                if self.show_about {
                    return;
                }
                let pressed = state == winit::event::ElementState::Pressed;
                // Space: hold = temporary hand tool; tap (no drag) = reset view.
                let is_space = matches!(&logical_key, Key::Named(NamedKey::Space))
                    || matches!(&logical_key, Key::Character(c) if c.as_str() == " ");
                if is_space {
                    if pressed {
                        if self.heal.painting {
                            self.finish_heal_stroke();
                        }
                        self.space_held = true;
                        self.space_dragged = false;
                        self.update_cursor_icon();
                    } else {
                        self.space_held = false;
                        self.update_cursor_icon();
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
                            && primary_modifier_pressed(self.modifiers)
                            && self.modifiers.shift_key() =>
                    {
                        self.open_folder_dialog();
                    }
                    Key::Character(c)
                        if (c == "s" || c == "S")
                            && primary_modifier_pressed(self.modifiers)
                            && self.modifiers.shift_key() =>
                    {
                        self.save_as();
                    }
                    Key::Character(c)
                        if (c == "o" || c == "O") && primary_modifier_pressed(self.modifiers) =>
                    {
                        self.open_image_dialog();
                    }
                    Key::Character(c)
                        if (c == "z" || c == "Z") && primary_modifier_pressed(self.modifiers) =>
                    {
                        if self.modifiers.shift_key() {
                            self.redo_edit();
                        } else {
                            self.undo_edit();
                        }
                    }
                    Key::Character(c)
                        if (c == "y" || c == "Y") && primary_modifier_pressed(self.modifiers) =>
                    {
                        self.redo_edit();
                    }
                    Key::Character(c) if (c == "0") && primary_modifier_pressed(self.modifiers) => {
                        self.fit_to_view();
                    }
                    Key::Character(c) if (c == "1") && primary_modifier_pressed(self.modifiers) => {
                        self.set_actual_size();
                    }
                    Key::Character(c)
                        if (c == "+" || c == "=") && primary_modifier_pressed(self.modifiers) =>
                    {
                        self.zoom_at_viewport_center(1.15);
                    }
                    Key::Character(c)
                        if (c == "-" || c == "_") && primary_modifier_pressed(self.modifiers) =>
                    {
                        self.zoom_at_viewport_center(1.0 / 1.15);
                    }
                    Key::Character(c) if single_key_shortcut_allowed(self.modifiers) => {
                        self.handle_single_key_shortcut(c.as_str());
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.apply_crop_rect();
                    }
                    Key::Named(NamedKey::Escape) => {
                        if self.transform.is_cropping {
                            self.cancel_crop();
                        } else if self.heal.active {
                            self.toggle_heal_mode();
                        }
                    }
                    Key::Named(NamedKey::ArrowRight) if self.transform.is_cropping => {
                        self.adjust_crop_from_keyboard(1.0, 0.0);
                    }
                    Key::Named(NamedKey::ArrowLeft) if self.transform.is_cropping => {
                        self.adjust_crop_from_keyboard(-1.0, 0.0);
                    }
                    Key::Named(NamedKey::ArrowDown) if self.transform.is_cropping => {
                        self.adjust_crop_from_keyboard(0.0, 1.0);
                    }
                    Key::Named(NamedKey::ArrowUp) if self.transform.is_cropping => {
                        self.adjust_crop_from_keyboard(0.0, -1.0);
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
                    Key::Named(NamedKey::F5) => self.reload_current_image(),
                    Key::Named(NamedKey::F11) => self.toggle_fullscreen(),
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                    renderer.window().request_redraw();
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.set_mode(self.theme_preference.resolve(Some(theme)));
                    renderer.window().request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(probe) = self.performance_probe.as_mut()
                    && probe.idle_until.is_some()
                {
                    probe.idle_redraws = probe.idle_redraws.saturating_add(1);
                }
                let mut ui_actions = Vec::new();
                if let Some(until) = self.toast_until
                    && Instant::now() > until
                {
                    self.toast = None;
                    self.toast_until = None;
                }
                // Snapshot UI/transform state before exclusive borrow of the renderer.
                let scale_factor = self
                    .renderer
                    .as_ref()
                    .map_or(1.0, |renderer| renderer.window().scale_factor());
                let (heal_stroke_screen, heal_cursor_screen, heal_brush_screen_radius) =
                    self.heal_overlay_geometry(scale_factor);
                let crop_screen = self
                    .crop_screen_rect()
                    .and_then(|rect| crate::view::physical_rect_to_logical(rect, scale_factor));
                let playlist_pos = self
                    .playlist
                    .as_ref()
                    .map(|p| (p.index.saturating_add(1), p.files.len().max(1)));
                if self.show_filmstrip_panel && self.filmstrip_panel_open {
                    self.request_thumbs_for_filmstrip();
                }
                self.poll_thumbnails();
                let filmstrip = self.filmstrip_entries();
                let is_flagged = self
                    .loaded_image_path
                    .as_ref()
                    .is_some_and(|p| self.flags.contains(p));
                let flag_count = self.flags.len();
                let toast = self.toast.clone();
                let is_loading = self.image_loader_rx.is_some() || self.preview_worker.is_some();
                let load_error = self.load_error.clone();
                let save_busy = self.save_worker.is_some();
                let crop_busy = self.crop_worker.is_some();
                let path_str = self
                    .loaded_image_path
                    .as_ref()
                    .or(self.image_path.as_ref())
                    .map(|p| p.to_string_lossy().into_owned());
                let show_image_info = self.show_image_info;
                let show_tools_panel = self.show_tools_panel;
                let tools_panel_open = self.tools_panel_open;
                let tools_panel_side = self.tools_panel_side;
                let show_filmstrip_panel = self.show_filmstrip_panel;
                let filmstrip_panel_open = self.filmstrip_panel_open;
                let image_info_side = self.image_info_side;
                let retain_exif = self.retain_exif;
                let theme_preference = self.theme_preference;
                let show_about = self.show_about;
                let source_image_size = self
                    .current_image
                    .as_ref()
                    .map(|image| (image.width, image.height));
                let animation =
                    self.animation
                        .as_ref()
                        .map(|playback| crate::ui::AnimationUiInfo {
                            frame_index: playback.frame_index(),
                            frame_count: playback.frame_count(),
                            is_playing: playback.is_playing(),
                        });
                let details = self.image_details.clone();
                let color_profile = self.current_image.as_ref().map(|image| image.color_profile);
                let is_cropping = self.transform.is_cropping;
                let crop_ratio = self.transform.crop_ratio;
                let custom_crop_ratio = self.custom_crop_ratio;
                let is_healing = self.heal.active;
                let heal_busy = self.heal.is_busy();
                let heal_brush_radius = self.heal.brush_radius;
                let heal_feather_percent = self.heal.feather_percent;
                let heal_source = self
                    .heal
                    .refresh
                    .as_ref()
                    .map(|refresh| (refresh.candidate_index, refresh.candidate_count));
                let can_undo_edit =
                    !heal_busy && !crop_busy && !save_busy && self.heal.history.can_undo();
                let can_redo_edit =
                    !heal_busy && !crop_busy && !save_busy && self.heal.history.can_redo();
                let is_panning =
                    self.transform.is_panning || (self.space_held && self.mouse_left_down);
                let bg_override = self.bg_override;
                let theme_mode = self.theme_preference.resolve(
                    self.renderer
                        .as_ref()
                        .and_then(|renderer| renderer.window().theme()),
                );
                let zoom_t = self.transform.zoom;
                let offset_x = self.transform.offset_x;
                let offset_y = self.transform.offset_y;
                let rot_steps = self.transform.rotation_steps;
                let flip_h = self.transform.flip_h;
                let flip_v = self.transform.flip_v;
                let crop_rect = self.transform.crop_rect;
                let viewport_insets = self.viewport_insets();
                let pixel_scale = self.renderer.as_ref().and_then(|renderer| {
                    let size = renderer.window().inner_size();
                    let image = renderer.image_size()?;
                    let rotated90 = rot_steps.rem_euclid(2) != 0;
                    Some(
                        crate::view::fit_pixel_scale(
                            (size.width, size.height),
                            image,
                            rotated90,
                            viewport_insets,
                        ) * zoom_t,
                    )
                });
                let image_viewport = self.renderer.as_ref().and_then(|renderer| {
                    let size = renderer.window().inner_size();
                    crate::view::safe_viewport_rect((size.width, size.height), viewport_insets)
                });
                let logical_image_viewport =
                    image_viewport.and_then(|viewport| viewport.logical_bounds(scale_factor));
                let performance_image_path = self.loaded_image_path.clone();

                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };

                if let Some(bg) = bg_override {
                    renderer.set_clear_color(bg);
                } else {
                    renderer.set_mode(theme_mode);
                }

                let placement = if let Some(size) = renderer.image_size() {
                    let win_size = renderer.window().inner_size();
                    let rotated90 = rot_steps.rem_euclid(2) != 0;
                    let mut p = crate::view::fit_to_viewport(
                        (win_size.width, win_size.height),
                        size,
                        rotated90,
                        viewport_insets,
                    );

                    p.scale[0] *= zoom_t;
                    p.scale[1] *= zoom_t;
                    p.offset[0] += offset_x;
                    p.offset[1] += offset_y;

                    p.uv_matrix = crate::view::uv_transform(rot_steps, flip_h, flip_v);

                    if let Some(cr) = crop_rect {
                        p.crop_rect = cr;
                    }

                    Some(p)
                } else {
                    None
                };

                let img_size = renderer.image_size();
                let can_heal = !is_loading
                    && load_error.is_none()
                    && !crop_busy
                    && !save_busy
                    && image_is_fully_displayed(source_image_size, renderer.image_texture_size());
                let frame = crate::ui::UiFrameOwned {
                    show_image_info,
                    retain_exif,
                    background_override: bg_override,
                    theme_preference,
                    theme_mode,
                    show_about,
                    show_tools_panel,
                    tools_panel_open,
                    tools_panel_side,
                    show_filmstrip_panel,
                    filmstrip_panel_open,
                    image_info_side,
                    file_path: path_str,
                    img_size,
                    animation,
                    details,
                    color_profile,
                    is_cropping,
                    crop_ratio,
                    custom_crop_ratio,
                    is_healing,
                    can_heal,
                    heal_busy,
                    heal_brush_radius,
                    heal_feather_percent,
                    heal_source,
                    can_undo_edit,
                    can_redo_edit,
                    is_panning,
                    is_flagged,
                    flag_count,
                    has_image: img_size.is_some(),
                    is_loading,
                    load_error,
                    save_busy,
                    crop_busy,
                    playlist_pos,
                    pixel_scale: pixel_scale.unwrap_or(0.0),
                    toast,
                    filmstrip,
                    crop_screen,
                    crop_uv: crop_rect,
                    crop_swaps_axes: rot_steps.rem_euclid(2) != 0,
                    image_viewport: logical_image_viewport,
                    heal_stroke_screen,
                    heal_cursor_screen,
                    heal_brush_screen_radius,
                    context_menu_pos: self.context_menu_pos,
                };

                let presents_image = placement.is_some();
                let frame_output = renderer.render(placement, image_viewport, |ui| {
                    ui_actions = crate::ui::render(ui, &frame);
                });
                match frame_output.result {
                    FrameResult::Presented | FrameResult::Skipped => {}
                    FrameResult::NeedsReconfigure => renderer.reconfigure(),
                }
                if let Some(repaint_after) = frame_output.repaint_after {
                    self.egui_repaint_at = repaint_deadline(Instant::now(), repaint_after);
                }
                if frame_output.result == FrameResult::Presented
                    && let Some(probe) = self.performance_probe.as_mut()
                {
                    let now = Instant::now();
                    probe.record_window_ready(now);
                    if presents_image && let Some(path) = performance_image_path.as_deref() {
                        probe.record_presented_image(path, now);
                    }
                }

                for action in ui_actions {
                    match action {
                        crate::ui::UiAction::Open => {
                            self.open_image_dialog();
                        }
                        crate::ui::UiAction::OpenFolder => self.open_folder_dialog(),
                        crate::ui::UiAction::Reload => self.reload_current_image(),
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
                        crate::ui::UiAction::SetTheme(preference) => {
                            self.theme_preference = preference;
                            let save_error = crate::theme::save_preference(preference).err();
                            if let Some(renderer) = self.renderer.as_mut() {
                                renderer.window().set_theme(preference.window_theme());
                                let mode = preference.resolve(renderer.window().theme());
                                renderer.set_mode(mode);
                                renderer.window().request_redraw();
                            }
                            if let Some(error) = save_error {
                                self.show_toast(format!(
                                    "Appearance changed for this session but could not be remembered: {error}"
                                ));
                            }
                        }
                        crate::ui::UiAction::ShowAbout => {
                            self.show_about = true;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::CloseAbout => {
                            self.show_about = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::ToggleImageInfo => {
                            self.show_image_info = !self.show_image_info;
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleToolsPanelVisibility => {
                            self.show_tools_panel = !self.show_tools_panel;
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleToolsPanelExpansion => {
                            self.tools_panel_open = !self.tools_panel_open;
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleFilmstripPanelVisibility => {
                            self.show_filmstrip_panel = !self.show_filmstrip_panel;
                            if self.show_filmstrip_panel && self.filmstrip_panel_open {
                                self.request_thumbs_for_filmstrip();
                            }
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleFilmstripPanelExpansion => {
                            self.filmstrip_panel_open = !self.filmstrip_panel_open;
                            if self.show_filmstrip_panel && self.filmstrip_panel_open {
                                self.request_thumbs_for_filmstrip();
                            }
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::SetToolsPanelSide(side) => {
                            self.tools_panel_side = side;
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::SetImageInfoSide(side) => {
                            self.image_info_side = side;
                            if let Some(renderer) = self.renderer.as_ref() {
                                renderer.window().request_redraw();
                            }
                        }
                        crate::ui::UiAction::ToggleRetainExif => {
                            self.retain_exif = !self.retain_exif;
                            self.show_toast(if self.retain_exif {
                                "Saved copies will keep camera metadata (session only)"
                            } else {
                                "Saved copies will strip camera metadata (default)"
                            });
                        }
                        crate::ui::UiAction::ToggleAnimationPlayback => {
                            self.toggle_animation_playback();
                        }
                        crate::ui::UiAction::RetryLoad => {
                            self.retry_current_image_load();
                        }
                        crate::ui::UiAction::RotateCw => {
                            self.rotate_current(1);
                        }
                        crate::ui::UiAction::RotateCcw => {
                            self.rotate_current(-1);
                        }
                        crate::ui::UiAction::FlipH => {
                            self.flip_current_horizontally();
                        }
                        crate::ui::UiAction::FlipV => {
                            self.flip_current_vertically();
                        }
                        crate::ui::UiAction::ToggleFullscreen => {
                            self.is_fullscreen = !self.is_fullscreen;
                            if let Some(renderer) = self.renderer.as_mut() {
                                renderer.window().request_redraw();
                                if self.is_fullscreen {
                                    renderer.window().set_fullscreen(Some(
                                        winit::window::Fullscreen::Borderless(None),
                                    ));
                                } else {
                                    renderer.window().set_fullscreen(None);
                                }
                            }
                        }
                        crate::ui::UiAction::FitToView => self.fit_to_view(),
                        crate::ui::UiAction::ActualSize => self.set_actual_size(),
                        crate::ui::UiAction::ZoomIn => self.zoom_at_viewport_center(1.15),
                        crate::ui::UiAction::ZoomOut => {
                            self.zoom_at_viewport_center(1.0 / 1.15);
                        }
                        crate::ui::UiAction::Navigate(d) => self.navigate(d),
                        crate::ui::UiAction::NavigateTo(i) => self.navigate_to(i),
                        crate::ui::UiAction::ToggleCrop => {
                            self.toggle_crop_mode();
                        }
                        crate::ui::UiAction::ApplyCrop => {
                            self.apply_crop_rect();
                        }
                        crate::ui::UiAction::CancelCrop => {
                            self.cancel_crop();
                        }
                        crate::ui::UiAction::SetCropRatio(r) => {
                            self.set_crop_ratio(r);
                        }
                        crate::ui::UiAction::SetCustomCropRatio(width, height) => {
                            self.custom_crop_ratio = (width.max(1), height.max(1));
                        }
                        crate::ui::UiAction::SwapCropRatio => {
                            self.swap_crop_ratio();
                        }
                        crate::ui::UiAction::MoveCrop { pointer, delta } => {
                            self.move_crop_from_logical_pointer(pointer, delta);
                        }
                        crate::ui::UiAction::ResizeCrop {
                            handle_center,
                            pointer,
                        } => {
                            self.resize_crop_from_logical_pointer(handle_center, pointer);
                        }
                        crate::ui::UiAction::ToggleHeal => self.toggle_heal_mode(),
                        crate::ui::UiAction::CloseContextMenu => {
                            self.context_menu_pos = None;
                        }
                        crate::ui::UiAction::SetHealBrushRadius(radius) => {
                            self.set_heal_brush_radius(radius);
                        }
                        crate::ui::UiAction::SetHealFeather(percent) => {
                            self.set_heal_feather(percent);
                        }
                        crate::ui::UiAction::RefreshHealSource => {
                            self.refresh_heal_source();
                        }
                        crate::ui::UiAction::UndoEdit => self.undo_edit(),
                        crate::ui::UiAction::RedoEdit => self.redo_edit(),
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::OpenFile(path) => self.open_file_request(path),
            UserEvent::Wake => {}
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            UserEvent::AccessKit(event) => {
                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };
                if event.window_id != renderer.window().id() {
                    return;
                }
                match event.window_event {
                    accesskit_winit::WindowEvent::InitialTreeRequested => {
                        renderer.egui_ctx.enable_accesskit();
                        renderer.window().request_redraw();
                    }
                    accesskit_winit::WindowEvent::ActionRequested(request) => {
                        renderer.queue_accessibility_action(request);
                        renderer.window().request_redraw();
                    }
                    accesskit_winit::WindowEvent::AccessibilityDeactivated => {
                        renderer.egui_ctx.disable_accesskit();
                    }
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_thumbnails();
        self.poll_prefetch();
        self.poll_heal_result();
        self.poll_preview_result();
        self.poll_auxiliary_load();
        self.poll_crop_result();
        self.poll_save_result();
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
                    let message = format!("Could not decode: {e}");
                    self.load_error = Some(message.clone());
                    self.show_toast(format!(
                        "{message}. The previous image remains visible; Retry is available."
                    ));
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
            if self.finish_folder_scan(scan)
                && let Some(renderer) = self.renderer.as_ref()
            {
                renderer.window().request_redraw();
            }
        }

        self.advance_animation(Instant::now());

        self.advance_performance_probe(event_loop);
        let probe_repaint_at = self
            .performance_probe
            .as_ref()
            .filter(|probe| probe.outcome.is_none())
            .map(|probe| {
                probe
                    .idle_until
                    .unwrap_or(probe.deadline)
                    .min(probe.deadline)
            });
        let animation_repaint_at = self
            .animation
            .as_ref()
            .and_then(crate::animated::AnimationPlayback::next_deadline);
        let next_repaint = [
            self.egui_repaint_at,
            self.toast_until,
            probe_repaint_at,
            animation_repaint_at,
        ]
        .into_iter()
        .flatten()
        .min();
        match next_repaint {
            Some(deadline) if deadline <= Instant::now() => {
                if self.egui_repaint_at.is_some_and(|at| at <= deadline) {
                    self.egui_repaint_at = None;
                }
                if let Some(renderer) = self.renderer.as_ref() {
                    renderer.window().request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
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

fn filmstrip_range(index: usize, len: usize) -> std::ops::Range<usize> {
    let Some(last_index) = len.checked_sub(1) else {
        return 0..0;
    };
    let current = index.min(last_index);
    current.saturating_sub(4)..current.saturating_add(5).min(len)
}

fn performance_navigation_targets(current: usize, len: usize) -> VecDeque<usize> {
    let mut targets = VecDeque::new();
    if len <= 1 {
        return targets;
    }
    for index in [len / 4, len / 2, len.saturating_mul(3) / 4, len - 1] {
        let index = index.min(len - 1);
        if index != current && !targets.contains(&index) {
            targets.push_back(index);
        }
    }
    targets
}

fn repaint_deadline(now: Instant, repaint_after: Duration) -> Option<Instant> {
    if repaint_after == Duration::MAX {
        None
    } else {
        now.checked_add(repaint_after)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn assert_rect_close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-5,
                "expected {expected}, got {actual}"
            );
        }
    }

    fn assert_pair_close(actual: (f32, f32), expected: (f32, f32)) {
        assert!((actual.0 - expected.0).abs() < 1e-5);
        assert!((actual.1 - expected.1).abs() < 1e-5);
    }

    #[test]
    fn heal_stroke_exit_is_terminal_and_point_collection_is_bounded() {
        let point = crate::heal::StrokePoint { x: 4.0, y: 5.0 };
        let mut stroke = vec![point];
        assert_eq!(
            append_heal_stroke_point(&mut stroke, None, DEFAULT_HEAL_BRUSH_RADIUS),
            HealStrokeUpdate::LeftImage
        );
        assert_eq!(stroke, vec![point]);

        let mut full = vec![point; crate::heal::MAX_STROKE_POINTS];
        assert_eq!(
            append_heal_stroke_point(
                &mut full,
                Some(crate::heal::StrokePoint { x: 100.0, y: 100.0 }),
                DEFAULT_HEAL_BRUSH_RADIUS,
            ),
            HealStrokeUpdate::TooManyPoints
        );
        assert_eq!(full.len(), crate::heal::MAX_STROKE_POINTS);
    }

    #[test]
    fn canceling_a_heal_retains_and_invalidates_the_single_worker() {
        let (_sender, result_rx) = mpsc::channel::<HealWorkerOutput>();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut heal = HealTool {
            worker: Some(HealWorker {
                result_rx,
                cancel: cancel.clone(),
                apply_result: true,
                replacing_latest: false,
                previous_candidate_index: 0,
            }),
            ..HealTool::default()
        };

        heal.cancel_worker();

        assert!(heal.is_busy());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!heal.worker.as_ref().unwrap().apply_result);
    }

    #[test]
    fn test_load_icon() {
        assert!(load_icon().is_some(), "load_icon returned None!");
    }

    #[test]
    fn consumed_keyboard_routing_preserves_shortcuts_without_hijacking_controls() {
        use winit::keyboard::{Key, NamedKey};

        assert!(route_consumed_keyboard_key(
            &Key::Character("t".into()),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("+".into()),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("j".into()),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Character("z".into()),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowRight),
            true,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            true,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::Escape),
            false,
            true,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowRight),
            false,
            false,
        ));
        assert!(route_consumed_keyboard_key(
            &Key::Named(NamedKey::F5),
            false,
            false,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::ArrowDown),
            false,
            true,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Enter),
            true,
            false,
        ));
        assert!(!route_consumed_keyboard_key(
            &Key::Named(NamedKey::Space),
            false,
            false,
        ));
        assert!(single_key_shortcut_allowed(ModifiersState::default()));
        assert!(single_key_shortcut_allowed(ModifiersState::SHIFT));
        assert!(!single_key_shortcut_allowed(ModifiersState::CONTROL));
        assert!(!single_key_shortcut_allowed(ModifiersState::ALT));
        assert!(!single_key_shortcut_allowed(ModifiersState::SUPER));
    }

    #[test]
    fn spot_heal_requires_the_complete_source_image_on_the_gpu() {
        assert!(image_is_fully_displayed(
            Some((16_384, 8_192)),
            Some((16_384, 8_192))
        ));
        assert!(!image_is_fully_displayed(
            Some((32_768, 8_192)),
            Some((16_384, 8_192))
        ));
        assert!(!image_is_fully_displayed(Some((1, 1)), None));
        assert!(!image_is_fully_displayed(None, None));
    }

    #[test]
    fn filmstrip_window_is_bounded_for_edges_and_stale_indices() {
        assert_eq!(filmstrip_range(0, 0), 0..0);
        assert_eq!(filmstrip_range(99, 0), 0..0);
        assert_eq!(filmstrip_range(0, 20), 0..5);
        assert_eq!(filmstrip_range(10, 20), 6..15);
        assert_eq!(filmstrip_range(99, 20), 15..20);
    }

    #[test]
    fn performance_probe_samples_bounded_distinct_folder_positions() {
        assert!(performance_navigation_targets(0, 0).is_empty());
        assert!(performance_navigation_targets(0, 1).is_empty());
        assert_eq!(
            performance_navigation_targets(0, 100),
            VecDeque::from([25, 50, 75, 99])
        );
        assert_eq!(
            performance_navigation_targets(5, 8),
            VecDeque::from([2, 4, 6, 7])
        );
        assert_eq!(performance_navigation_targets(1, 2), VecDeque::from([0]));
    }

    #[test]
    fn unsettled_ui_restarts_the_idle_observation_window() {
        let mut probe = PerformanceProbe::new(Instant::now());
        probe.idle_until = Some(Instant::now());
        probe.idle_redraws = 42;

        probe.reset_idle_observation();

        assert_eq!(probe.idle_until, None);
        assert_eq!(probe.idle_redraws, 0);
    }

    #[test]
    fn egui_repaint_delay_becomes_an_event_loop_deadline() {
        let now = Instant::now();
        assert_eq!(repaint_deadline(now, Duration::MAX), None);
        assert_eq!(repaint_deadline(now, Duration::ZERO), Some(now));
        assert_eq!(
            repaint_deadline(now, Duration::from_millis(500)),
            now.checked_add(Duration::from_millis(500))
        );
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

    #[test]
    fn default_free_crop_has_predictable_ten_percent_margins() {
        assert_rect_close(
            default_crop_rect((400, 200), CropRatio::Free),
            [0.1, 0.1, 0.9, 0.9],
        );
    }

    #[test]
    fn default_square_crop_accounts_for_non_square_pixels_in_uv_space() {
        assert_rect_close(
            default_crop_rect((400, 200), CropRatio::SQUARE),
            [0.3, 0.1, 0.7, 0.9],
        );
    }

    #[test]
    fn all_standard_crop_presets_hold_their_pixel_aspect() {
        let presets = [
            CropRatio::SQUARE,
            CropRatio::THREE_TWO,
            CropRatio::TWO_THREE,
            CropRatio::FOUR_THREE,
            CropRatio::THREE_FOUR,
            CropRatio::FIVE_FOUR,
            CropRatio::FOUR_FIVE,
            CropRatio::FIVE_THREE,
            CropRatio::THREE_FIVE,
            CropRatio::SIXTEEN_NINE,
            CropRatio::NINE_SIXTEEN,
        ];
        for image_size in [(4032, 3024), (3024, 4032), (1001, 667)] {
            for ratio in presets {
                let rect = default_crop_rect(image_size, ratio);
                let pixel_width = (rect[2] - rect[0]) * image_size.0 as f32;
                let pixel_height = (rect[3] - rect[1]) * image_size.1 as f32;
                let expected = crop_pixel_aspect(image_size, ratio).unwrap();
                assert!(
                    (pixel_width / pixel_height - expected).abs() < 1e-4,
                    "{} did not hold for {image_size:?}",
                    ratio.label()
                );
            }
        }
    }

    #[test]
    fn locked_crop_exports_exact_integer_ratios_on_odd_dimensions() {
        let presets = [
            CropRatio::SQUARE,
            CropRatio::THREE_TWO,
            CropRatio::TWO_THREE,
            CropRatio::FOUR_THREE,
            CropRatio::THREE_FOUR,
            CropRatio::FIVE_FOUR,
            CropRatio::FOUR_FIVE,
            CropRatio::FIVE_THREE,
            CropRatio::THREE_FIVE,
            CropRatio::SIXTEEN_NINE,
            CropRatio::NINE_SIXTEEN,
            CropRatio::fixed(7, 11),
        ];
        for image_size in [(101, 101), (1001, 667), (4031, 3023)] {
            for ratio in presets {
                let uv = default_crop_rect(image_size, ratio);
                let pixel = App::crop_pixel_rect(uv, image_size.0, image_size.1, ratio)
                    .unwrap_or_else(|| panic!("{} did not fit {image_size:?}", ratio.label()));
                let (ratio_width, ratio_height) = crop_integer_ratio(image_size, ratio).unwrap();
                assert_eq!(
                    u64::from(pixel.width) * u64::from(ratio_height),
                    u64::from(pixel.height) * u64::from(ratio_width),
                    "{} did not quantize exactly for {image_size:?}",
                    ratio.label()
                );
                assert!(pixel.x + pixel.width <= image_size.0);
                assert!(pixel.y + pixel.height <= image_size.1);
            }
        }

        let rect = default_crop_rect((101, 101), CropRatio::SIXTEEN_NINE);
        let pixel = App::crop_pixel_rect(rect, 101, 101, CropRatio::SIXTEEN_NINE).unwrap();
        assert_eq!((pixel.width, pixel.height), (80, 45));
    }

    #[test]
    fn crop_pixel_quantization_handles_free_edges_rotation_and_tiny_images() {
        let free = App::crop_pixel_rect([0.8, 0.8, 1.0, 1.0], 101, 101, CropRatio::Free).unwrap();
        assert_eq!(
            free,
            crate::edit::Rect {
                x: 80,
                y: 80,
                width: 21,
                height: 21
            }
        );

        let source_ratio = crop_ratio_for_source(CropRatio::SIXTEEN_NINE, 1);
        let rect = default_crop_rect((101, 151), source_ratio);
        let rotated = App::crop_pixel_rect(rect, 101, 151, source_ratio).unwrap();
        assert_eq!(u64::from(rotated.width) * 16, u64::from(rotated.height) * 9);
        assert!(
            App::crop_pixel_rect([0.0, 0.0, 1.0, 1.0], 15, 8, CropRatio::SIXTEEN_NINE).is_none()
        );
    }

    #[test]
    fn fixed_crop_ratio_tracks_the_visible_orientation() {
        let image_size = (4_000, 3_000);
        let source_ratio = crop_ratio_for_source(CropRatio::SIXTEEN_NINE, 1);
        assert_eq!(source_ratio, CropRatio::NINE_SIXTEEN);
        let rect = default_crop_rect(image_size, source_ratio);
        let source_width = (rect[2] - rect[0]) * image_size.0 as f32;
        let source_height = (rect[3] - rect[1]) * image_size.1 as f32;
        assert!((source_height / source_width - 16.0 / 9.0).abs() < 1e-4);
        assert_eq!(
            crop_ratio_for_source(CropRatio::Original, 1),
            CropRatio::Original
        );
        assert_eq!(
            crop_ratio_for_source(CropRatio::FOUR_FIVE, 2),
            CropRatio::FOUR_FIVE
        );
    }

    #[test]
    fn original_crop_ratio_tracks_each_image() {
        for image_size in [(4032, 3024), (3024, 4032), (1001, 667)] {
            let rect = default_crop_rect(image_size, CropRatio::Original);
            let pixel_width = (rect[2] - rect[0]) * image_size.0 as f32;
            let pixel_height = (rect[3] - rect[1]) * image_size.1 as f32;
            assert!(
                (pixel_width / pixel_height - image_size.0 as f32 / image_size.1 as f32).abs()
                    < 1e-4
            );
        }
    }

    #[test]
    fn image_dimensions_reduce_to_a_stable_custom_ratio() {
        assert_eq!(reduced_crop_ratio(4032, 3024), Some((4, 3)));
        assert_eq!(reduced_crop_ratio(3024, 4032), Some((3, 4)));
        assert_eq!(reduced_crop_ratio(0, 10), None);
    }

    #[test]
    fn crop_handle_hit_mapping_uses_source_geometry() {
        let rect = [0.2, 0.3, 0.8, 0.9];
        assert_eq!(crop_handle_from_uv(rect, (0.2, 0.3)), CropHandle::TopLeft);
        assert_eq!(crop_handle_from_uv(rect, (0.5, 0.3)), CropHandle::Top);
        assert_eq!(crop_handle_from_uv(rect, (0.8, 0.6)), CropHandle::Right);
        assert_eq!(
            crop_handle_from_uv(rect, (0.2, 0.9)),
            CropHandle::BottomLeft
        );
    }

    #[test]
    fn free_crop_handles_move_only_the_expected_edges() {
        let rect = [0.2, 0.2, 0.8, 0.8];
        assert_rect_close(
            resize_crop_rect_from_pointer(
                rect,
                (400, 300),
                CropRatio::Free,
                CropHandle::TopLeft,
                (0.1, 0.15),
            ),
            [0.1, 0.15, 0.8, 0.8],
        );
        assert_rect_close(
            resize_crop_rect_from_pointer(
                rect,
                (400, 300),
                CropRatio::Free,
                CropHandle::Right,
                (0.9, 0.4),
            ),
            [0.2, 0.2, 0.9, 0.8],
        );
    }

    #[test]
    fn every_locked_crop_handle_preserves_ratio_and_bounds() {
        let handles = [
            CropHandle::TopLeft,
            CropHandle::Top,
            CropHandle::TopRight,
            CropHandle::Right,
            CropHandle::BottomRight,
            CropHandle::Bottom,
            CropHandle::BottomLeft,
            CropHandle::Left,
        ];
        let image_size = (400, 300);
        let ratio = CropRatio::SIXTEEN_NINE;
        let initial = default_crop_rect(image_size, ratio);
        let expected = crop_pixel_aspect(image_size, ratio).unwrap();
        for handle in handles {
            for pointer in [(-0.2, -0.1), (0.15, 0.25), (0.95, 0.85), (1.2, 1.1)] {
                let rect =
                    resize_crop_rect_from_pointer(initial, image_size, ratio, handle, pointer);
                assert!(rect[0] >= -1e-6 && rect[1] >= -1e-6);
                assert!(rect[2] <= 1.0 + 1e-6 && rect[3] <= 1.0 + 1e-6);
                assert!(rect[2] > rect[0] && rect[3] > rect[1]);
                let pixel_width = (rect[2] - rect[0]) * image_size.0 as f32;
                let pixel_height = (rect[3] - rect[1]) * image_size.1 as f32;
                assert!(
                    (pixel_width / pixel_height - expected).abs() < 1e-4,
                    "{handle:?} produced {rect:?}"
                );
            }
        }
    }

    #[test]
    fn keyboard_crop_move_preserves_size_and_clamps_at_image_edge() {
        let moved = adjust_crop_rect(
            [0.1, 0.2, 0.5, 0.6],
            (400, 200),
            CropRatio::Free,
            -0.5,
            0.8,
            false,
        );
        assert_rect_close(moved, [0.0, 0.6, 0.4, 1.0]);
    }

    #[test]
    fn keyboard_crop_resize_preserves_locked_pixel_aspect() {
        let resized = adjust_crop_rect(
            [0.3, 0.1, 0.7, 0.9],
            (400, 200),
            CropRatio::SQUARE,
            0.1,
            0.0,
            true,
        );
        let pixel_width = (resized[2] - resized[0]) * 400.0;
        let pixel_height = (resized[3] - resized[1]) * 200.0;
        assert!((pixel_width / pixel_height - 1.0).abs() < 1e-5);
    }

    #[test]
    fn keyboard_crop_direction_tracks_rotation_and_flip_on_screen() {
        assert_pair_close(
            crop_keyboard_delta(1.0, 0.0, crate::view::uv_transform(1, false, false), false),
            (0.0, -1.0),
        );
        assert_pair_close(
            crop_keyboard_delta(1.0, 0.0, crate::view::uv_transform(0, true, false), false),
            (-1.0, 0.0),
        );
        assert_pair_close(
            crop_keyboard_delta(1.0, 0.0, crate::view::uv_transform(0, true, false), true),
            (1.0, 0.0),
        );
    }
}
