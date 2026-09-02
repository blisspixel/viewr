//! The application: a message loop of our own on winit's event loop. For Phase 0
//! it opens a window, sets up the GPU renderer, and clears each frame to the
//! theme background. The Elm-style shape (one state, messages, update, render)
//! is borrowed without depending on a UI framework.
#![allow(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::crop::{
    CropRatio, adjust_crop_rect, crop_handle_from_uv, crop_keyboard_delta, crop_pixel_aspect,
    crop_ratio_for_source, default_crop_rect, fit_crop_rect_to_ratio, reduced_crop_ratio,
    resize_crop_rect_from_pointer,
};
use crate::performance::{
    PERFORMANCE_IDLE_OBSERVATION, PERFORMANCE_PROBE_TIMEOUT, PerformanceProbe,
    schedule_performance_wake,
};
use crate::ratings::{
    RatingAssignment, RatingFilter, RatingObservation, RatingState, RatingWriteCapability,
    RatingWriteError,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowAttributes, WindowId};

use crate::crop_state::{
    CropRecoveryIdentity, crop_disconnect_message, crop_failure_message,
    crop_preview_disconnect_message, crop_recovery_blocker, crop_recovery_matches,
    crop_source_blocker, preview_retry_blocker,
};
use crate::curate::{GuardedActionError, TrashRestoreDisposition, TrashedFile};
use crate::curation_state::{
    CurationCloseDisposition, CurationKind, CurationRecovery, CurationTerminalState,
    GuardedSourceAction, PERMANENT_DELETE_ACTION, curation_close_disposition,
    curation_recovery_message, curation_status, guarded_source_action_failure_message,
    permanent_delete_confirmed, permanent_delete_description, permanent_delete_success_message,
    restore_result_message, single_trash_result_message,
};
use crate::current_work::{
    ActiveModeAllowance, CurrentWork, blocked_action_message, browse_work_blocker, crop_work,
    curation_action_preflight, curation_work, current_work_blocker, image_preparation_work,
    spot_heal_source_blocker, spot_heal_work,
};
use crate::decode::{DecodedImage, LoadedImage};
use crate::edit_state::edit_transaction_failure_message;
use crate::entry_state::{
    FolderScanDisposition, FolderScanSuccess, PathEntry, folder_scan_disposition,
    folder_scan_failure_class, folder_scan_user_message, path_entry, selected_file_index_by,
    selected_scan_is_current,
};
use crate::error::Error;
use crate::gpu::{FrameResult, ImagePreview, Renderer};
use crate::job::{JobPoll, OneShotJob};
#[cfg(test)]
use crate::keyboard_route::route_consumed_keyboard_key;
use crate::keyboard_route::{
    EscapeAction, EscapeContext, escape_action, escape_press_reaches_app, is_fullscreen_toggle_key,
    is_space_key, is_trash_shortcut_key, rating_assignment_for_key, rating_keys_apply,
    repeated_viewer_action_allowed, route_consumed_keyboard_key_in_context,
    single_key_shortcut_allowed, space_press_starts_hold, space_release_must_unwind,
    space_tap_fits, widget_popup_owns_event,
};
use crate::playlist::{FilterSelection, Playlist, ScanPurpose, filter_selection_changes_source};
use crate::prefetch::{
    self, PrefetchCache, PrefetchDestination, path_free_texture_id, prefetch_destination,
};
use crate::presentation::{
    ImageReuseEligibility, NavigationImagePlan, PresentationKind, PresentedFrameTransition,
    decode_failure_toast, durable_presentation_error, external_edit_pending_after_frame_transition,
    image_open_in_progress, navigation_image_plan, preview_job_matches, user_facing_decode_error,
};
use crate::rating_state::{
    PresentedRatingTransition, RatingCloseDisposition, RatingDiscoveryTransition,
    RatingRecoveryTransition, RatingWriteTerminal, auxiliary_disconnect_message,
    next_presented_rating, next_rating_recovery_state, rating_after_auxiliary_disconnect,
    rating_close_disposition, rating_discovery_transition, rating_recovery_after_presentation,
    rating_recovery_blocker, rating_write_discovery_blocker, rating_write_failure_message,
    rating_write_target_is_current, reconcile_rating_write,
};
use crate::save_state::{
    CloseDisposition, SaveCloseDisposition, SaveStartBlocker, SaveTerminalState, close_disposition,
    folder_scan_blocks_save, save_close_disposition, save_start_blocker,
    save_start_blocker_message,
};
use crate::session::{
    ForegroundLoadFailure, ForegroundLoadFailureDisposition, ForegroundRetryPlan,
    foreground_retry_plan, resolve_foreground_load_failure,
};
use crate::theme::{Preference, PreferenceRecovery, appearance_save_failure_message};
use crate::thumbs::{self, ThumbnailCompletion};
use crate::ui::FilmstripItem;
use crate::work_currency::{loaded_work_is_current, presented_work_is_current};

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

#[allow(
    clippy::too_many_lines,
    reason = "startup assembles the complete explicit application state in one place"
)]
fn run_internal(
    image_path: Option<PathBuf>,
    performance_probe: Option<PerformanceProbe>,
) -> Result<Option<crate::performance::PerformanceReport>, Error> {
    // A desktop viewer that cannot reach a window says so before starting the
    // event loop, instead of aborting inside the dynamic loader.
    crate::startup::preflight().map_err(Error::Launch)?;
    let event_loop = build_event_loop()?;
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
    let image_path = image_path.map(|path| crate::fs::canonical_file_path(&path).unwrap_or(path));
    let probe_enabled = performance_probe.is_some();
    let appearance = crate::theme::load_preference();
    let appearance_recovery = appearance.recovery();
    if let Some(recovery) = appearance_recovery {
        log::warn!(
            "appearance preference fallback: {}",
            recovery.diagnostic_name()
        );
    }
    let folder_sort = crate::folder_sort_preference::load();
    let folder_sort_recovery = folder_sort.recovery();
    if let Some(recovery) = folder_sort_recovery {
        log::warn!(
            "folder sort preference fallback: {}",
            recovery.diagnostic_name()
        );
    }
    let mut app = App {
        session: crate::session::Session {
            selected_path: image_path,
            ..Default::default()
        },
        renderer: None,
        display_monitor: None,
        display_hints: initial_display_hints(),
        display_profile_usable: false,
        playlist: None,
        playlist_scope: None,
        folder_sort: folder_sort.sort(),
        folder_scan_job: None,
        rating_scan_worker: None,
        rating_generation: 0,
        rating_write_disclosed: false,
        pending_rating_write: None,
        rating_write_worker: None,
        close_after_rating_write: false,
        rating_recovery_unsettled: false,
        current_rating_capability: RatingWriteCapability::UnsafeSource,
        presented_rating: RatingState::Loading,
        transform: Transform::default(),
        custom_crop_ratio: (3, 5),
        heal: HealTool::default(),
        tools_before_heal: None,
        is_fullscreen: false,
        mosaic: MosaicView::default(),
        last_trashed: Vec::new(),
        last_trashed_scope: None,
        current_image: None,
        current_preview: None,
        current_source: None,
        current_image_reuse: ImageReuseEligibility::Ineligible,
        animation: None,
        pages: None,
        image_details: None,
        auxiliary_job: None,
        open_with_job: None,
        coherence_watch: None,
        unsaved_crop: false,
        pending_gone_notice: false,
        last_coherence_action: None,
        pending_save: None,
        save_job: None,
        close_after_save: false,
        save_recovery_unsettled: false,
        crop_job: None,
        crop_recovery_unsettled: false,
        preview_job: None,
        preview_recovery_unsettled: false,
        preview_load_retry_blocked: false,
        curation_worker: None,
        close_after_curation: false,
        curation_recovery: CurationRecovery::default(),
        show_image_info: false,
        show_tools_panel: false,
        tools_panel_open: true,
        tools_panel_side: crate::chrome::DockSide::Left,
        show_filmstrip_panel: probe_enabled,
        filmstrip_panel_open: true,
        image_info_side: crate::chrome::DockSide::Right,
        // Privacy default: Save As strips EXIF/GPS unless the user opts in.
        retain_exif: false,
        bg_override: None,
        theme_preference: appearance.preference(),
        preference_recovery_notice: startup_preference_recovery_notice(
            appearance_recovery,
            folder_sort_recovery,
        ),
        show_about: false,
        show_update: false,
        show_preferences: false,
        show_file_associations: false,
        external_edit_pending: false,
        source_gone: false,
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
        thumbnail_schedule: thumbs::ThumbnailSchedule::default(),
        thumb_textures: HashMap::new(),
        prefetch: PrefetchCache::with_limits(
            prefetch::DEFAULT_CAPACITY,
            prefetch::DEFAULT_MAX_BYTES,
        ),
        prefetch_sources: HashMap::new(),
        prefetch_schedule: prefetch::PrefetchSchedule::default(),
        event_proxy,
        performance_probe,
        startup_failure: None,
    };
    if let Some(path) = app.session.selected_path.clone() {
        app.open_path_request(path);
    }
    event_loop.run_app(&mut app)?;
    if let Some(failure) = app.startup_failure.take() {
        return Err(failure);
    }
    let Some(probe) = app.performance_probe else {
        return Ok(None);
    };
    probe
        .outcome
        .unwrap_or_else(|| Err("performance probe exited before completion".into()))
        .map(Some)
        .map_err(Error::Platform)
}

/// Create the event loop, pinned to the backend viewr resolved for this host.
///
/// Pinning matters when `WAYLAND_DISPLAY` survives from an earlier session:
/// winit would bind Wayland and fail, while the X server named by `DISPLAY` is
/// running. A failure is reported as one packaged sentence with no path from
/// the machine that built viewr.
fn build_event_loop() -> Result<EventLoop<UserEvent>, Error> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "linux")]
    {
        use winit::platform::wayland::EventLoopBuilderExtWayland as _;
        use winit::platform::x11::EventLoopBuilderExtX11 as _;

        match crate::startup::preferred_backend() {
            Some(crate::startup::DisplaySession::X11) => {
                builder.with_x11();
            }
            Some(crate::startup::DisplaySession::Wayland) => {
                builder.with_wayland();
            }
            _ => {}
        }
    }
    builder.build().map_err(|error| {
        Error::Launch(crate::startup::host_event_loop_failure_message(
            &error.to_string(),
        ))
    })
}

/// Host color-management facts that do not change after the window backend is chosen.
fn initial_display_hints() -> crate::display_state::DisplayHints {
    crate::display_state::DisplayHints {
        os: crate::display_state::current_os(),
        advanced_color: None,
        session: host_display_session(),
    }
}

fn host_display_session() -> crate::display_state::DisplaySession {
    #[cfg(target_os = "linux")]
    {
        let support = crate::startup::resolve_window_support();
        let backend = match support.session {
            crate::startup::DisplaySession::Wayland => {
                crate::display_state::LinuxWindowBackend::Wayland
            }
            crate::startup::DisplaySession::X11 => crate::display_state::LinuxWindowBackend::X11,
            crate::startup::DisplaySession::None => crate::display_state::LinuxWindowBackend::None,
        };
        let wayland_reachable = match support.session {
            crate::startup::DisplaySession::Wayland => true,
            crate::startup::DisplaySession::X11 => {
                !support.compositor_unreachable
                    && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
            }
            crate::startup::DisplaySession::None => false,
        };
        crate::display_state::linux_session(backend, wayland_reachable)
    }
    #[cfg(not(target_os = "linux"))]
    {
        crate::display_state::DisplaySession::Native
    }
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

struct FolderScanContext {
    purpose: Option<ScanPurpose>,
    cancel: Arc<AtomicBool>,
}

enum MissingSelectionRemoval {
    Advance(PathBuf),
    ScanFolder,
    FilterEmpty,
}

impl Drop for FolderScanContext {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

struct RatingScanWorker {
    generation: u64,
    cancel: Arc<AtomicBool>,
    result_rx: Receiver<Vec<(PathBuf, RatingState)>>,
}

struct PendingRatingWrite {
    path: PathBuf,
    assignment: RatingAssignment,
}

fn cancel_pending_rating_for_source_change(
    pending_rating_write: &mut Option<PendingRatingWrite>,
) -> bool {
    pending_rating_write.take().is_some()
}

struct RatingWriteWorker {
    path: PathBuf,
    assignment: RatingAssignment,
    result_rx: Receiver<Result<crate::ratings::VerifiedRatingWrite, RatingWriteError>>,
    join: JoinHandle<()>,
}

/// Exact identity of one installed playlist. Restores may rejoin only this view.
#[derive(Debug)]
struct PlaylistScope;

#[derive(Default)]
struct MosaicView {
    page: Option<crate::mosaic::MosaicPage>,
    uploaded_paths: Vec<Option<PathBuf>>,
    unavailable_paths: HashSet<PathBuf>,
    memory_limited: bool,
    display_limited: bool,
}

impl MosaicView {
    fn is_active(&self) -> bool {
        self.page.is_some()
    }
}

struct CropJobContext {
    recovery: CropRecovery,
    cancel: Arc<AtomicBool>,
}

enum CropJobResult {
    Completed(DecodedImage),
    Failed(String),
    Cancelled,
}

struct PreviewJobContext {
    path: PathBuf,
    generation: u64,
    kind: PresentationKind,
    source: Option<Arc<crate::fs::ImageSource>>,
    crop_recovery: Option<CropRecovery>,
}

enum PreviewJobResult {
    Prepared(Arc<DecodedImage>, ImagePreview),
    Failed(String),
    Cancelled,
}

struct RemovalContext {
    path: PathBuf,
    playlist_index: usize,
    scope: Option<Arc<PlaylistScope>>,
}

struct RestoreContext {
    submitted: usize,
    scope: Option<Arc<PlaylistScope>>,
}

enum CurationContext {
    Trash(RemovalContext),
    PermanentDelete(RemovalContext),
    Restore(RestoreContext),
}

impl CurationContext {
    const fn kind(&self) -> CurationKind {
        match self {
            Self::Trash(_) => CurationKind::Trash,
            Self::PermanentDelete(_) => CurationKind::PermanentDelete,
            Self::Restore(_) => CurationKind::Restore,
        }
    }

    const fn submitted(&self) -> usize {
        match self {
            Self::Trash(_) | Self::PermanentDelete(_) => 1,
            Self::Restore(context) => context.submitted,
        }
    }
}

struct RestoredEntryEvidence {
    path: PathBuf,
    rating: RatingState,
    provenance: Option<crate::fs::ScanProvenance>,
}

enum CurationCompletion {
    Trash {
        result: Result<crate::curate::TrashReceipt, GuardedActionError>,
    },
    PermanentDelete {
        result: Result<(), GuardedActionError>,
    },
    Restore {
        outcome: crate::curate::TrashRestoreOutcome,
        evidence: Vec<RestoredEntryEvidence>,
        elapsed: Duration,
    },
}

struct CurationWorker {
    context: CurationContext,
    result_rx: Receiver<CurationCompletion>,
    join: Option<JoinHandle<()>>,
}

impl CurationWorker {
    fn status(&self, closing: bool) -> String {
        curation_status(self.context.kind(), self.context.submitted(), closing)
    }
}

struct AuxiliaryLoadContext {
    path: PathBuf,
    generation: u64,
}

struct OpenWithContext {
    path: PathBuf,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

struct CoherenceWatch {
    path: PathBuf,
    cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<crate::file_coherence::CoherenceObservation>>>,
}

enum AuxiliarySequence {
    None,
    Animation(crate::animated::DecodedAnimation),
    Pages(crate::pages::DecodedPages),
}

type AuxiliaryLoadResult = (
    Result<AuxiliarySequence, String>,
    crate::image_info::ImageDetails,
    RatingObservation,
);

type SaveResult = Result<crate::edit::MetadataDisposition, String>;

struct PendingSave {
    source_path: PathBuf,
    source_image: Arc<DecodedImage>,
    source: Option<Arc<crate::fs::ImageSource>>,
    destination: crate::edit::SaveDestination,
    pixel_transform: crate::edit::PixelTransform,
    options: crate::edit::SaveOptions,
}

fn cancel_pending_save_for_source_change(pending_save: &mut Option<PendingSave>) -> bool {
    pending_save.take().is_some()
}

struct CropRecovery {
    source_path: PathBuf,
    source_generation: u64,
    source_image: Arc<DecodedImage>,
    transform: Transform,
    animation: Option<crate::animated::AnimationPlayback>,
    pages: Option<crate::pages::PageCursor>,
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
}

struct RestoredCropEditState {
    transform: Transform,
    animation: Option<crate::animated::AnimationPlayback>,
    pages: Option<crate::pages::PageCursor>,
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
}

impl CropRecovery {
    fn into_restored_edit_state(self) -> RestoredCropEditState {
        let Self {
            mut transform,
            animation,
            pages,
            auxiliary_job,
            ..
        } = self;
        transform.crop_start = None;
        RestoredCropEditState {
            transform,
            animation,
            pages,
            auxiliary_job,
        }
    }
}

enum WorkerPoll<T> {
    Pending,
    Ready(T),
    Disconnected,
}

fn poll_worker<T>(receiver: &Receiver<T>) -> WorkerPoll<T> {
    match receiver.try_recv() {
        Ok(result) => WorkerPoll::Ready(result),
        Err(mpsc::TryRecvError::Empty) => WorkerPoll::Pending,
        Err(mpsc::TryRecvError::Disconnected) => WorkerPoll::Disconnected,
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

fn log_guarded_action_failure(action: GuardedSourceAction, error: &GuardedActionError) {
    let action = match action {
        GuardedSourceAction::Trash => "trash",
        GuardedSourceAction::PermanentDelete => "permanent_delete",
    };
    if matches!(error, GuardedActionError::OperationFailed(_)) {
        log::error!(
            "source-bound file action failed: action={action}, category={}",
            error.category()
        );
    } else {
        log::warn!(
            "source-bound file action rejected: action={action}, category={}",
            error.category()
        );
    }
}

fn spawn_curation_thread<T: Send + 'static>(
    name: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
    wake: impl FnOnce() + Send + 'static,
) -> Result<(Receiver<T>, JoinHandle<()>), ()> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            match outcome {
                Ok(result) => {
                    let _ = sender.send(result);
                    drop(sender);
                    wake();
                }
                Err(payload) => {
                    drop(sender);
                    wake();
                    std::panic::resume_unwind(payload);
                }
            }
        })
        .map(|join| (receiver, join))
        .map_err(|_| ())
}

fn commit_presented_heal<E>(
    image: &mut DecodedImage,
    history: &mut crate::heal::PatchHistory,
    refresh: &mut Option<HealRefresh>,
    result: &crate::heal::SpotHealResult,
    job: Option<crate::heal::SpotHealJob>,
    replacing_latest: bool,
    present: impl FnOnce(&DecodedImage, &crate::heal::ImagePatch) -> Result<(), E>,
) -> Result<String, crate::heal::PatchPresentationError<E>> {
    let inverse = crate::heal::apply_presented_patch(image, &result.patch, present)?;
    if !replacing_latest {
        history.record(inverse);
    }
    *refresh = job.and_then(|job| {
        (result.candidate_count > 1).then_some(HealRefresh {
            job,
            candidate_index: result.candidate_index,
            candidate_count: result.candidate_count,
        })
    });
    Ok(heal_success_message(
        replacing_latest,
        result.candidate_index,
        result.candidate_count,
    ))
}

fn heal_success_message(
    replacing_latest: bool,
    candidate_index: usize,
    candidate_count: usize,
) -> String {
    if replacing_latest {
        format!("Heal source {} of {}", candidate_index + 1, candidate_count)
    } else {
        "Spot healed in memory. Use Save As to keep it; Undo is available.".to_owned()
    }
}

fn save_success_message(
    metadata: crate::edit::MetadataDisposition,
    includes_pixel_edits: bool,
) -> String {
    let copy = if includes_pixel_edits {
        "Saved edited copy"
    } else {
        "Saved copy"
    };
    let metadata = match metadata {
        crate::edit::MetadataDisposition::Retained => "EXIF retained",
        crate::edit::MetadataDisposition::NotPresent => "no EXIF found",
        crate::edit::MetadataDisposition::Stripped => "metadata stripped",
    };
    format!("{copy} · {metadata}")
}

fn startup_preference_recovery_notice(
    appearance: Option<PreferenceRecovery>,
    folder_sort: Option<crate::folder_sort_preference::Recovery>,
) -> Option<&'static str> {
    match (appearance, folder_sort) {
        (Some(_), Some(_)) => Some(
            "Could not restore saved appearance or folder sort. Using System and Latest First.",
        ),
        (Some(recovery), None) => Some(recovery.notice()),
        (None, Some(recovery)) => Some(recovery.notice()),
        (None, None) => None,
    }
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
    display_monitor: Option<crate::display_state::MonitorIdentity>,
    display_hints: crate::display_state::DisplayHints,
    display_profile_usable: bool,
    session: crate::session::Session,
    playlist: Option<Playlist>,
    playlist_scope: Option<Arc<PlaylistScope>>,
    /// Persisted default folder order used by the current playlist.
    folder_sort: crate::fs::FolderSort,
    folder_scan_job: Option<
        OneShotJob<
            FolderScanContext,
            Result<Vec<crate::fs::ScannedImage>, crate::fs::ScanImagesError>,
        >,
    >,
    /// Cancellable, generation-tagged in-memory rating discovery for one folder.
    rating_scan_worker: Option<RatingScanWorker>,
    /// Monotonic owner token for folder rating results.
    rating_generation: u64,
    /// Session-only acknowledgement of embedded source mutation.
    rating_write_disclosed: bool,
    /// First write awaiting explicit disclosure confirmation.
    pending_rating_write: Option<PendingRatingWrite>,
    /// At most one atomic rating replacement transaction.
    rating_write_worker: Option<RatingWriteWorker>,
    /// A normal close request waiting for rating replacement to reconcile.
    close_after_rating_write: bool,
    /// An indeterminate source mutation that requires a trusted reload boundary.
    rating_recovery_unsettled: bool,
    /// Write capability associated with the currently presented source.
    current_rating_capability: RatingWriteCapability,
    /// Rating associated with the last source whose pixels were presented.
    presented_rating: RatingState,
    transform: Transform,
    /// Last custom crop ratio entered during this process session.
    custom_crop_ratio: (u16, u16),
    heal: HealTool,
    /// Tools visibility captured when Spot Heal opened the dock.
    tools_before_heal: Option<(bool, bool)>,
    is_fullscreen: bool,
    /// Transient, memory-bounded full-image mosaic. Never persisted.
    mosaic: MosaicView,
    last_trashed: Vec<TrashedFile>,
    last_trashed_scope: Option<Arc<PlaylistScope>>,
    current_image: Option<Arc<DecodedImage>>,
    current_preview: Option<ImagePreview>,
    /// Live handle for the exact source object that supplied the displayed pixels.
    current_source: Option<Arc<crate::fs::ImageSource>>,
    /// Whether the displayed pixels are a pristine source decode safe to cache.
    current_image_reuse: ImageReuseEligibility,
    /// Timed frames for the current GIF, WebP, or APNG.
    animation: Option<crate::animated::AnimationPlayback>,
    /// Still pages for the current multi-page TIFF or multi-size ICO.
    pages: Option<crate::pages::PageCursor>,
    /// Best-effort facts for the current Image Information panel.
    image_details: Option<crate::image_info::ImageDetails>,
    /// Replace-latest animation and metadata result for the current source.
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
    /// Generation-cancellable source verification before the native Open With chooser.
    open_with_job: Option<OneShotJob<OpenWithContext, crate::fs::ImageSourceMatch>>,
    coherence_watch: Option<CoherenceWatch>,
    /// A crop has been applied to the presented pixels and is not on disk.
    unsaved_crop: bool,
    /// A gone source is waiting for the folder refresh before speaking.
    pending_gone_notice: bool,
    last_coherence_action: Option<crate::file_coherence::CoherenceAction>,
    /// Captured existing destination awaiting object-bound overwrite consent.
    pending_save: Option<PendingSave>,
    /// At most one explicit Save As encode with bounded completion ownership.
    save_job: Option<OneShotJob<(), SaveResult>>,
    /// A normal close request waiting for Save As to reach a terminal state.
    close_after_save: bool,
    /// An indeterminate Save As worker loss that requires a process restart.
    save_recovery_unsettled: bool,
    /// At most one full-resolution crop with bounded completion ownership.
    crop_job: Option<OneShotJob<CropJobContext, CropJobResult>>,
    /// An indeterminate crop worker loss that requires a process restart.
    crop_recovery_unsettled: bool,
    /// Replace-latest over-limit preview with bounded completion ownership.
    preview_job: Option<OneShotJob<PreviewJobContext, PreviewJobResult>>,
    /// A lost preview executor completion that requires a process restart.
    preview_recovery_unsettled: bool,
    /// The current load cannot retry because it requires the lost preview executor.
    preview_load_retry_blocked: bool,
    /// At most one source-bound Trash, permanent-delete, or restore operation.
    curation_worker: Option<CurationWorker>,
    /// A normal close request waiting for destructive work to reconcile.
    close_after_curation: bool,
    /// Durable, operation-bound guidance after indeterminate worker loss.
    curation_recovery: CurationRecovery,
    show_image_info: bool,
    /// Whether the tools dock reserves any viewport space.
    show_tools_panel: bool,
    /// Whether the docked tools panel is expanded.
    tools_panel_open: bool,
    /// Horizontal edge used by the tools dock.
    tools_panel_side: crate::chrome::DockSide,
    /// Whether folder previews reserve any viewport space.
    show_filmstrip_panel: bool,
    /// Whether the docked folder-preview panel is expanded.
    filmstrip_panel_open: bool,
    /// Horizontal edge used by Image Information.
    image_info_side: crate::chrome::DockSide,
    /// When true, Save As copies EXIF from the source. Default **false** (strip).
    retain_exif: bool,
    bg_override: Option<[f64; 4]>,
    /// Persisted application-chrome and default-canvas appearance.
    theme_preference: Preference,
    /// Abnormal preference fallback to announce once the first window is ready.
    preference_recovery_notice: Option<&'static str>,
    /// Whether the accessible About window is open.
    show_about: bool,
    /// Whether the accessible local update-instructions window is open.
    show_update: bool,
    /// Whether the accessible persistent-preferences window is open.
    show_preferences: bool,
    /// Whether the accessible opt-in file-association guide is open.
    show_file_associations: bool,
    /// Whether another app may have changed the source since the last accepted decode.
    external_edit_pending: bool,
    /// Whether the selected path no longer names the presented file.
    source_gone: bool,
    /// Latest keyboard modifiers (for Shift+Delete, etc.).
    modifiers: ModifiersState,
    /// Transient outcome message shown in chrome when chrome is visible.
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
    /// Bounded owner for thumbnail generations, jobs, and terminal failures.
    thumbnail_schedule: thumbs::ThumbnailSchedule,
    /// Uploaded egui textures for filmstrip cells.
    thumb_textures: HashMap<PathBuf, egui::TextureHandle>,
    /// In-memory neighbor full-decode cache (never written to disk).
    prefetch: PrefetchCache,
    /// Source handles paired with every path retained by the decoded-image cache.
    prefetch_sources: HashMap<PathBuf, Arc<crate::fs::ImageSource>>,
    /// Bounded owners, generations, cancellation, and terminal prefetch state.
    prefetch_schedule: prefetch::PrefetchSchedule,
    /// Wakes the event loop when background work finishes before a window exists.
    event_proxy: EventLoopProxy<UserEvent>,
    /// Explicit developer/CI performance probe; absent from normal launches.
    performance_probe: Option<PerformanceProbe>,
    /// Why the window or its GPU surface never appeared. Reported on exit so a
    /// failed launch is never a silent success.
    startup_failure: Option<Error>,
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

fn application_shortcuts_blocked<const N: usize>(owners: [bool; N]) -> bool {
    owners.into_iter().any(|owner| owner)
}

fn modal_dispatch_allows(
    owns_dispatch: &mut bool,
    modal_open: bool,
    action: &crate::ui::UiAction,
    action_allowed: fn(&crate::ui::UiAction) -> bool,
) -> bool {
    *owns_dispatch |= modal_open;
    !*owns_dispatch || action_allowed(action)
}

const fn filmstrip_is_available(projected_count: usize) -> bool {
    projected_count > 1
}

fn restore_targets_active_playlist(
    playlist: Option<&Playlist>,
    active: Option<&Arc<PlaylistScope>>,
    trashed: Option<&Arc<PlaylistScope>>,
) -> bool {
    playlist.is_some() && same_playlist_scope(active, trashed)
}

fn inspect_restored_entries(
    outcome: &mut crate::curate::TrashRestoreOutcome,
) -> Vec<RestoredEntryEvidence> {
    outcome.restored.sort_by_key(|record| record.playlist_index);
    outcome
        .restored
        .iter()
        .map(|record| {
            let path = record.receipt.original_path().to_owned();
            let restored_source = record.receipt.open_restored_source();
            let rating = restored_source
                .as_ref()
                .map_or(RatingState::Unreadable, |source| {
                    crate::ratings::observe_source(source, &path).state
                });
            let provenance = restored_source
                .as_ref()
                .and_then(crate::fs::ImageSource::scan_provenance)
                .or_else(|| {
                    record
                        .receipt
                        .restore_source()
                        .and_then(crate::fs::ImageSource::current_scan_provenance)
                });
            RestoredEntryEvidence {
                path,
                rating,
                provenance,
            }
        })
        .collect()
}

fn same_playlist_scope(
    active: Option<&Arc<PlaylistScope>>,
    trashed: Option<&Arc<PlaylistScope>>,
) -> bool {
    active
        .zip(trashed)
        .is_some_and(|(active, trashed)| Arc::ptr_eq(active, trashed))
}

fn rebase_preserved_trash_action(
    records: &mut [TrashedFile],
    active: Option<&Arc<PlaylistScope>>,
    trashed: Option<&Arc<PlaylistScope>>,
    removed_indices: &[usize],
) {
    if same_playlist_scope(active, trashed) {
        crate::curate::rebase_trashed_file_indices_after_current_removals(records, removed_indices);
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

#[allow(clippy::needless_pass_by_value)]
fn run_coherence_watch(
    path: PathBuf,
    source: Arc<crate::fs::ImageSource>,
    folder: Option<PathBuf>,
    mut folder_stamp: Option<crate::fs::DirectoryStamp>,
    cancel: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<crate::file_coherence::CoherenceObservation>>>,
    event_proxy: EventLoopProxy<UserEvent>,
) {
    use crate::file_coherence::{
        CoherenceObservation, FolderObservation, merge_observation, source_observation,
    };
    let mut last_sent = None;
    while !cancel.load(Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(400));
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let source = source_observation(source.matches_path(&path));
        let folder = match folder.as_deref() {
            None => FolderObservation::Unchanged,
            Some(folder) => match crate::fs::directory_stamp(folder) {
                None => FolderObservation::Unavailable,
                Some(stamp) => {
                    let changed = folder_stamp.is_some_and(|previous| previous != stamp);
                    folder_stamp = Some(stamp);
                    if changed {
                        FolderObservation::Changed
                    } else {
                        FolderObservation::Unchanged
                    }
                }
            },
        };
        let observation = CoherenceObservation { source, folder };
        if matches!(
            (observation.source, observation.folder),
            (
                crate::file_coherence::SourceObservation::Unchanged,
                crate::file_coherence::FolderObservation::Unchanged
            )
        ) {
            last_sent = Some(observation);
            continue;
        }
        if last_sent == Some(observation) {
            continue;
        }
        last_sent = Some(observation);
        if let Ok(mut pending) = latest.lock() {
            *pending = Some(match pending.take() {
                None => observation,
                Some(previous) => merge_observation(previous, observation),
            });
        } else {
            return;
        }
        let _ = event_proxy.send_event(UserEvent::Wake);
    }
}

impl App {
    /// Open one path delivered by the command line, a drop, or the desktop.
    fn open_path_request(&mut self, path: PathBuf) {
        match path_entry(&path, Path::is_dir) {
            PathEntry::Folder => {
                if self.block_action_while_curating("opening another folder") {
                    return;
                }
                self.cancel_open_with_check();
                let directory = crate::fs::canonical_file_path(&path).unwrap_or(path);
                self.start_folder_scan(directory, ScanPurpose::OpenFolder);
            }
            PathEntry::Image => self.load_and_scan(path),
        }
    }

    fn load_and_scan(&mut self, path: PathBuf) {
        if self.block_action_while_curating("opening another image") {
            return;
        }
        let path = crate::fs::canonical_file_path(&path).unwrap_or(path);
        let missing_recovery = self.session.selected_missing
            && self.session.selected_path.as_deref() == Some(path.as_path());
        self.reset_prefetch_for_playlist_change();
        self.playlist = None;
        self.playlist_scope = None;
        self.begin_image_load(path.clone(), missing_recovery);
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        self.start_folder_scan(
            directory,
            ScanPurpose::SelectedFile {
                path,
                missing_recovery,
            },
        );
    }

    fn begin_image_load(&mut self, path: PathBuf, preserve_missing_recovery: bool) {
        self.cancel_open_with_check();
        self.stop_coherence_watch();
        self.pending_gone_notice = false;
        self.source_gone = false;
        self.cancel_save_overwrite_for_source_change();
        self.cancel_rating_disclosure_for_source_change();
        self.session.selected_path = Some(path.clone());
        self.transform = Transform::default();
        self.spawn_image_load_with_recovery(path, preserve_missing_recovery);
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn open_image_dialog(&mut self) {
        if self.block_action_while_curating("opening another image") {
            return;
        }
        self.cancel_open_with_check();
        let extensions = crate::fs::supported_extensions().collect::<Vec<_>>();
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &extensions)
            .pick_file()
        {
            self.load_and_scan(path);
        }
    }

    fn open_folder_dialog(&mut self) {
        if self.block_action_while_curating("opening another folder") {
            return;
        }
        self.cancel_open_with_check();
        if let Some(directory) = rfd::FileDialog::new().pick_folder() {
            self.start_folder_scan(directory, ScanPurpose::OpenFolder);
        }
    }

    fn start_folder_scan(&mut self, directory: PathBuf, purpose: ScanPurpose) {
        self.folder_scan_job = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let event_proxy = self.event_proxy.clone();
        let folder_sort = self.folder_sort;
        let (completion, job) = OneShotJob::new(
            FolderScanContext {
                purpose: Some(purpose),
                cancel,
            },
            move || {
                let _ = event_proxy.send_event(UserEvent::Wake);
            },
        );
        let spawn_result = std::thread::Builder::new()
            .name("viewr-folder-scan".into())
            .spawn(move || {
                let files =
                    crate::fs::scan_image_entries_sorted_while(&directory, folder_sort, || {
                        !worker_cancel.load(Ordering::Acquire)
                    });
                let _ = completion.complete(files);
            });
        match spawn_result {
            Ok(_) => self.folder_scan_job = Some(job),
            Err(error) => {
                log::error!("failed to start folder scan");
                self.show_toast(format!("Could not scan folder: {error}"));
            }
        }
    }

    fn reset_prefetch_for_playlist_change(&mut self) {
        self.mosaic = MosaicView::default();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_mosaic_images();
        }
        self.prefetch.clear();
        self.prefetch
            .set_limits(prefetch::DEFAULT_CAPACITY, prefetch::DEFAULT_MAX_BYTES);
        self.prefetch_sources.clear();
        self.prefetch_schedule.reset();
    }

    fn set_folder_sort(&mut self, sort: crate::fs::FolderSort) {
        if self.folder_sort != sort {
            self.folder_sort = sort;
            if let Some(playlist) = self.playlist.as_mut() {
                playlist.sort(sort);
                self.reset_prefetch_for_playlist_change();
                self.thumbnail_schedule.reset();
                self.thumb_textures.clear();
                self.kick_prefetch();
            }
        }
        match crate::folder_sort_preference::save(sort) {
            Ok(()) => self.show_toast(format!("Default folder sort: {}", sort.label())),
            Err(error) => {
                log::error!(
                    "failed to save folder sort preference: {}",
                    error.diagnostic_name()
                );
                self.show_toast(crate::folder_sort_preference::save_failure_message());
            }
        }
        self.request_redraw();
    }

    fn take_prefetched_image(&mut self, path: &Path) -> Option<LoadedImage> {
        let image = self.prefetch.take(path)?;
        let Some(source) = self.prefetch_sources.remove(path) else {
            log::error!("prefetch source invariant failed");
            return None;
        };
        Some(LoadedImage { image, source })
    }

    fn remove_prefetched_image(&mut self, path: &Path) {
        let _ = self.prefetch.take(path);
        self.prefetch_sources.remove(path);
    }

    fn insert_prefetched_image(&mut self, path: PathBuf, loaded: LoadedImage) -> bool {
        let retained = self.prefetch.insert(path.clone(), loaded.image);
        if retained {
            self.prefetch_sources.insert(path, loaded.source);
        }
        let prefetch = &self.prefetch;
        self.prefetch_sources
            .retain(|cached_path, _| prefetch.contains(cached_path));
        retained
    }

    fn insert_mosaic_image_if_fits(&mut self, path: PathBuf, loaded: LoadedImage) -> bool {
        let retained = self.prefetch.insert_if_fits(path.clone(), loaded.image);
        if retained {
            self.prefetch_sources.insert(path, loaded.source);
        }
        retained
    }

    fn replace_playlist(&mut self, files: Vec<PathBuf>, index: usize) {
        self.install_playlist(Playlist::new(files, index));
    }

    fn replace_playlist_from_scan(&mut self, entries: Vec<crate::fs::ScannedImage>, index: usize) {
        self.install_playlist(Playlist::from_scan(entries, index));
    }

    fn install_playlist(&mut self, playlist: Playlist) {
        if let Some(worker) = self.rating_scan_worker.take() {
            worker.cancel.store(true, Ordering::Release);
        }
        self.rating_generation = self.rating_generation.wrapping_add(1);
        self.reset_prefetch_for_playlist_change();
        self.thumbnail_schedule.reset();
        self.thumb_textures.clear();
        self.playlist_scope = Some(Arc::new(PlaylistScope));
        self.playlist = Some(playlist);
    }

    fn preserve_presented_source_provenance(&mut self, selected: &Path) {
        if self.session.presented_path.as_deref() != Some(selected) {
            return;
        }
        let Some(source) = self.current_source.as_deref() else {
            return;
        };
        if let Some(playlist) = self.playlist.as_mut() {
            bind_playlist_source_provenance(playlist, selected, source, false);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn finish_folder_scan(
        &mut self,
        purpose: ScanPurpose,
        files: Result<Vec<crate::fs::ScannedImage>, crate::fs::ScanImagesError>,
    ) -> bool {
        let open_folder = matches!(purpose, ScanPurpose::OpenFolder);
        let selected_is_current = match &purpose {
            ScanPurpose::SelectedFile { path, .. } => {
                selected_scan_is_current(self.session.selected_path.as_deref(), path)
            }
            ScanPurpose::OpenFolder => true,
        };
        let selected_missing = match &purpose {
            ScanPurpose::SelectedFile {
                missing_recovery, ..
            } => *missing_recovery || self.session.selected_missing,
            ScanPurpose::OpenFolder => false,
        };
        let success = files.as_ref().map(|entries| match &purpose {
            ScanPurpose::SelectedFile { path: selected, .. } => FolderScanSuccess::Selected {
                matched_index: selected_file_index_by(
                    entries,
                    selected,
                    crate::fs::ScannedImage::path,
                )
                .or_else(|| {
                    if selected_missing {
                        return None;
                    }
                    let current = self.current_source.as_ref()?.scan_provenance()?;
                    entries
                        .iter()
                        .position(|entry| current.same_object(entry.provenance()))
                }),
                count: entries.len(),
            },
            ScanPurpose::OpenFolder => FolderScanSuccess::OpenFolder {
                count: entries.len(),
            },
        });
        let result = match success {
            Ok(facts) => Ok(facts),
            Err(error) => Err(folder_scan_failure_class(
                matches!(error, crate::fs::ScanImagesError::Cancelled),
                matches!(
                    error,
                    crate::fs::ScanImagesError::LimitExceeded
                        | crate::fs::ScanImagesError::PathBudgetExceeded
                ),
            )),
        };
        let disposition =
            folder_scan_disposition(open_folder, selected_is_current, selected_missing, result);
        if matches!(disposition, FolderScanDisposition::Discard) {
            return false;
        }
        if let Some(message) = folder_scan_user_message(disposition) {
            if matches!(
                disposition,
                FolderScanDisposition::InstallSelectedOnlyScanFailed
                    | FolderScanDisposition::SelectedMissingScanFailed
                    | FolderScanDisposition::OpenFolderFailed
            ) && let Err(error) = &files
            {
                log::warn!("folder scan unavailable: {error}");
            }
            self.show_toast(message);
        }
        match (disposition, purpose, files) {
            (
                FolderScanDisposition::InstallScanAt(index),
                ScanPurpose::SelectedFile { path: selected, .. },
                Ok(entries),
            ) => {
                let reload_restored_selection = self.session.selected_missing;
                self.replace_playlist_from_scan(entries, index);
                let new_path = self
                    .playlist
                    .as_ref()
                    .and_then(|playlist| playlist.files.get(index).cloned());
                let followed_rename = new_path
                    .as_deref()
                    .is_some_and(|path| self.session.selected_path.as_deref() != Some(path));
                if let Some(new_path) = new_path
                    && followed_rename
                {
                    self.cancel_rating_disclosure_for_source_change();
                    self.session.selected_path = Some(new_path.clone());
                    if self.session.presented_path.is_some() {
                        self.session.presented_path = Some(new_path);
                    }
                    self.start_coherence_watch();
                }
                self.settle_pending_gone_notice(!followed_rename, followed_rename);
                let provenance_path = self
                    .session
                    .selected_path
                    .clone()
                    .unwrap_or_else(|| selected.clone());
                self.preserve_presented_source_provenance(&provenance_path);
                if reload_restored_selection {
                    self.spawn_image_load(provenance_path);
                }
                self.kick_prefetch();
            }
            (
                FolderScanDisposition::InstallSelectedOnly
                | FolderScanDisposition::InstallSelectedOnlyLimitExceeded
                | FolderScanDisposition::InstallSelectedOnlyScanFailed,
                ScanPurpose::SelectedFile { path: selected, .. },
                _,
            ) => {
                self.replace_playlist(vec![selected.clone()], 0);
                self.preserve_presented_source_provenance(&selected);
                self.settle_pending_gone_notice(false, false);
                if matches!(disposition, FolderScanDisposition::InstallSelectedOnly) {
                    self.kick_prefetch();
                }
            }
            (FolderScanDisposition::OpenFolderFirst, ScanPurpose::OpenFolder, Ok(entries))
            | (
                FolderScanDisposition::InstallScanFirstAfterSelectedMissing,
                ScanPurpose::SelectedFile { .. },
                Ok(entries),
            ) => {
                let first = entries[0].path().to_owned();
                self.replace_playlist_from_scan(entries, 0);
                self.begin_image_load(first, false);
                self.kick_prefetch();
            }
            (
                FolderScanDisposition::OpenFolderEmpty
                | FolderScanDisposition::OpenFolderLimitExceeded
                | FolderScanDisposition::OpenFolderFailed
                | FolderScanDisposition::SelectedMissing
                | FolderScanDisposition::SelectedMissingLimitExceeded
                | FolderScanDisposition::SelectedMissingScanFailed,
                _,
                _,
            ) => {}
            (FolderScanDisposition::Discard, _, _) => unreachable!("discard returned early"),
            _ => {
                log::error!("folder scan disposition mismatched purpose or payload");
            }
        }
        true
    }

    fn poll_folder_scan(&mut self) -> bool {
        let Some(completed_scan) = self.folder_scan_job.as_ref().map(OneShotJob::poll) else {
            return false;
        };
        if matches!(completed_scan, JobPoll::Pending) {
            return false;
        }
        let mut context = self
            .folder_scan_job
            .take()
            .expect("folder scan job exists after polling it")
            .into_context();
        let purpose = context
            .purpose
            .take()
            .expect("active folder scan retains its purpose");
        let files = match completed_scan {
            JobPoll::Ready(files) => files,
            JobPoll::Disconnected => Err(crate::fs::ScanImagesError::WorkerStopped),
            JobPoll::Pending => unreachable!("pending folder scan returned early"),
        };
        self.finish_folder_scan(purpose, files)
    }

    fn display_loaded_image(&mut self, path: &Path, loaded: LoadedImage) {
        if let Some(playlist) = self.playlist.as_mut() {
            bind_playlist_source_provenance(playlist, path, &loaded.source, true);
        }
        self.present_image(
            path,
            loaded.image,
            Some(loaded.source),
            PresentationKind::Loaded,
        );
    }

    fn present_image(
        &mut self,
        path: &Path,
        image: Arc<DecodedImage>,
        source: Option<Arc<crate::fs::ImageSource>>,
        kind: PresentationKind,
    ) {
        self.present_image_with_crop_recovery(path, image, source, kind, None);
    }

    fn present_cropped_image(
        &mut self,
        path: &Path,
        image: Arc<DecodedImage>,
        recovery: CropRecovery,
    ) {
        let source = self.current_source.clone();
        self.present_image_with_crop_recovery(
            path,
            image,
            source,
            PresentationKind::Cropped,
            Some(recovery),
        );
    }

    fn present_image_with_crop_recovery(
        &mut self,
        path: &Path,
        image: Arc<DecodedImage>,
        source: Option<Arc<crate::fs::ImageSource>>,
        kind: PresentationKind,
        crop_recovery: Option<CropRecovery>,
    ) {
        if self.mosaic.is_active() {
            self.leave_full_image_mosaic();
        }
        if crop_recovery
            .as_ref()
            .is_some_and(|recovery| !self.crop_recovery_is_current(recovery))
        {
            log::debug!("discarded stale crop before preview preparation");
            return;
        }
        let required = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.required_preview(&image));
        let Some(spec) = required else {
            self.finish_image_presentation(path, image, source, None, kind, crop_recovery);
            return;
        };
        if self.preview_recovery_unsettled {
            self.report_preview_executor_loss(kind, crop_recovery);
            return;
        }

        let generation = self.session.generation.load(Ordering::Acquire);
        let current_generation = Arc::clone(&self.session.generation);
        let worker_image = Arc::clone(&image);
        let context = PreviewJobContext {
            path: path.to_owned(),
            generation,
            kind,
            source,
            crop_recovery,
        };
        let notify_proxy = self.event_proxy.clone();
        let (completion, job) = OneShotJob::new(context, move || {
            let _ = notify_proxy.send_event(UserEvent::Wake);
        });
        let scheduled = crate::decode::schedule_image_preview(move || {
            let result = match crate::gpu::prepare_image_preview(&worker_image, spec, || {
                current_generation.load(Ordering::Acquire) != generation
            }) {
                Ok(Some(preview)) => PreviewJobResult::Prepared(worker_image, preview),
                Ok(None) => PreviewJobResult::Cancelled,
                Err(error) => PreviewJobResult::Failed(error.to_string()),
            };
            if !completion.complete(result) {
                log::debug!("discarded preview result after owner replacement");
            }
        });
        match scheduled {
            Ok(()) => {
                self.preview_job = Some(job);
                self.show_toast("Preparing a display-sized preview in the background");
            }
            Err(error) => {
                let context = job.into_context();
                if context.kind == PresentationKind::Cropped {
                    log::error!("crop preview queue failed: {error}");
                } else {
                    log::error!("failed to queue image preview: {error}");
                }
                self.report_preview_executor_loss(context.kind, context.crop_recovery);
            }
        }
    }

    fn report_preview_executor_loss(
        &mut self,
        kind: PresentationKind,
        crop_recovery: Option<CropRecovery>,
    ) {
        self.preview_recovery_unsettled = true;
        if kind == PresentationKind::Cropped {
            let restored = crop_recovery.is_some_and(|recovery| self.restore_failed_crop(recovery));
            self.show_toast(crop_preview_disconnect_message(restored));
            return;
        }
        self.preview_load_retry_blocked = true;
        self.session.load_error = Some(crate::ui::PREVIEW_RECOVERY_STATUS.to_owned());
        self.show_toast(crate::ui::PREVIEW_RECOVERY_STATUS);
    }

    fn report_presentation_failure(
        &mut self,
        kind: PresentationKind,
        message: String,
        crop_recovery: Option<CropRecovery>,
    ) {
        if kind == PresentationKind::Cropped {
            let restored = crop_recovery.is_some_and(|recovery| self.restore_failed_crop(recovery));
            self.show_toast(crop_failure_message(restored));
            return;
        }
        if let Some(load_error) = durable_presentation_error(kind, &message) {
            self.session.load_error = Some(load_error);
        }
        self.show_toast(message);
    }

    fn finish_image_presentation(
        &mut self,
        path: &Path,
        image: Arc<DecodedImage>,
        source: Option<Arc<crate::fs::ImageSource>>,
        preview: Option<&ImagePreview>,
        kind: PresentationKind,
        crop_recovery: Option<CropRecovery>,
    ) {
        if crop_recovery
            .as_ref()
            .is_some_and(|recovery| !self.crop_recovery_is_current(recovery))
        {
            log::debug!("discarded stale crop before renderer presentation");
            return;
        }
        let full_resolution = if let Some(renderer) = self.renderer.as_mut() {
            match renderer.set_image(&image, preview) {
                Ok(full_resolution) => full_resolution,
                Err(error) => {
                    if kind == PresentationKind::Cropped {
                        log::error!("crop renderer presentation failed: {error}");
                    } else {
                        log::error!("failed to upload prepared image: {error}");
                    }
                    self.report_presentation_failure(
                        kind,
                        format!("Could not display image: {error}"),
                        crop_recovery,
                    );
                    return;
                }
            }
        } else {
            true
        };
        self.current_image = Some(image);
        self.current_preview = preview.cloned();
        self.current_source = source;
        self.current_image_reuse = kind.image_reuse();
        let rating_transition = if self.session.presented_path.as_deref() == Some(path) {
            PresentedRatingTransition::Retain
        } else {
            PresentedRatingTransition::Replace(
                self.playlist
                    .as_ref()
                    .map_or(RatingState::Loading, |playlist| {
                        playlist.rating_for_path(path)
                    }),
            )
        };
        self.session.set_presented(path.to_owned());
        self.presented_rating = next_presented_rating(self.presented_rating, rating_transition);
        let recovery_transition = rating_recovery_after_presentation(
            matches!(kind, PresentationKind::Loaded),
            self.current_source.is_some(),
        );
        self.rating_recovery_unsettled =
            next_rating_recovery_state(self.rating_recovery_unsettled, recovery_transition);
        self.external_edit_pending = external_edit_pending_after_frame_transition(
            self.external_edit_pending,
            PresentedFrameTransition::Present(kind),
        );
        match kind {
            PresentationKind::Loaded => {
                self.unsaved_crop = false;
                self.source_gone = false;
                self.prefetch_schedule.allow(path);
                self.start_auxiliary_load(path);
            }
            PresentationKind::Cropped => {
                self.unsaved_crop = true;
                self.heal.reset_for_image();
                self.show_toast("Crop applied");
            }
        }
        self.start_coherence_watch();
        if !full_resolution {
            self.show_toast(
                "Full image shown as a GPU-limited preview; export remains full resolution",
            );
        }
    }

    fn poll_preview_result(&mut self) {
        let Some(job) = self.preview_job.as_ref() else {
            return;
        };
        let polled = job.poll();
        if matches!(&polled, JobPoll::Pending) {
            return;
        }
        let context = self
            .preview_job
            .take()
            .expect("preview job exists after polling it")
            .into_context();
        let PreviewJobContext {
            path,
            generation,
            kind,
            source,
            crop_recovery,
        } = context;
        if !preview_job_matches(
            generation,
            &path,
            self.session.generation.load(Ordering::Acquire),
            self.session.selected_path.as_deref(),
        ) {
            if kind == PresentationKind::Cropped {
                log::debug!("discarded stale crop preview result");
            }
            return;
        }
        match polled {
            JobPoll::Ready(PreviewJobResult::Prepared(image, preview)) => {
                self.finish_image_presentation(
                    &path,
                    image,
                    source,
                    Some(&preview),
                    kind,
                    crop_recovery,
                );
                self.request_redraw();
            }
            JobPoll::Ready(PreviewJobResult::Failed(error)) => {
                if kind == PresentationKind::Cropped {
                    log::error!("crop preview preparation failed: {error}");
                } else {
                    log::error!("image preview preparation failed: {error}");
                }
                self.report_presentation_failure(
                    kind,
                    format!("Could not prepare image preview: {error}"),
                    crop_recovery,
                );
            }
            JobPoll::Ready(PreviewJobResult::Cancelled) => {
                log::error!("current preview job reported unexpected cancellation");
                self.report_preview_executor_loss(kind, crop_recovery);
            }
            JobPoll::Disconnected => {
                if kind == PresentationKind::Cropped {
                    log::error!("crop preview job disconnected before publishing a result");
                } else {
                    log::error!("image preview job disconnected before publishing a result");
                }
                self.report_preview_executor_loss(kind, crop_recovery);
            }
            JobPoll::Pending => unreachable!("pending preview result returned early"),
        }
    }

    fn invalidate_displayed_image(&mut self) {
        self.cancel_rating_disclosure_for_source_change();
        self.external_edit_pending = external_edit_pending_after_frame_transition(
            self.external_edit_pending,
            PresentedFrameTransition::Invalidate,
        );
        self.heal.reset_for_image();
        self.cancel_crop_work();
        self.preview_job = None;
        self.preview_load_retry_blocked = false;
        self.animation = None;
        self.pages = None;
        self.image_details = None;
        self.auxiliary_job = None;
        self.session.load_error = None;
        self.stop_coherence_watch();
        self.unsaved_crop = false;
        self.pending_gone_notice = false;
        self.source_gone = false;
        self.last_coherence_action = None;
        self.current_image = None;
        self.current_preview = None;
        self.current_source = None;
        self.current_rating_capability = RatingWriteCapability::UnsafeSource;
        self.presented_rating =
            next_presented_rating(self.presented_rating, PresentedRatingTransition::Clear);
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
        self.session.presented_path = None;
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_image();
        }
    }

    /// Stop work and edit state tied to the old image while leaving its last
    /// good pixels on screen until a replacement has decoded successfully.
    fn prepare_for_image_load(&mut self, preserve_missing_recovery: bool) {
        self.external_edit_pending = external_edit_pending_after_frame_transition(
            self.external_edit_pending,
            PresentedFrameTransition::RetainForReplacement,
        );
        self.heal.reset_for_image();
        self.cancel_crop_work();
        self.preview_job = None;
        self.preview_load_retry_blocked = false;
        self.animation = None;
        self.pages = None;
        self.auxiliary_job = None;
        self.current_rating_capability = RatingWriteCapability::UnsafeSource;
        self.presented_rating =
            next_presented_rating(self.presented_rating, PresentedRatingTransition::Retain);
        self.rating_recovery_unsettled = next_rating_recovery_state(
            self.rating_recovery_unsettled,
            RatingRecoveryTransition::Retain,
        );
        self.session.prepare_for_load(preserve_missing_recovery);
    }

    fn start_auxiliary_load(&mut self, path: &Path) {
        self.animation = None;
        self.pages = None;
        self.image_details = None;
        self.auxiliary_job = None;
        let path = path.to_owned();
        let job_path = path.clone();
        let current_generation = Arc::clone(&self.session.generation);
        let generation = current_generation.load(Ordering::Acquire);
        let event_proxy = self.event_proxy.clone();
        let (completion, job) =
            OneShotJob::new(AuxiliaryLoadContext { path, generation }, move || {
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
        let source = self.current_source.clone();
        let scheduled = crate::decode::schedule_current_image_details(move || {
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let sequence = source
                .as_ref()
                .map_or(Ok(AuxiliarySequence::None), |source| {
                    match crate::animated::DecodedAnimation::load_background_if_current(
                        &job_path,
                        source,
                        &current_generation,
                        generation,
                    ) {
                        Ok(Some(animation)) => Ok(AuxiliarySequence::Animation(animation)),
                        Ok(None) => {
                            match crate::pages::DecodedPages::load_background_if_current(
                                &job_path,
                                source,
                                &current_generation,
                                generation,
                            ) {
                                Ok(Some(pages)) => Ok(AuxiliarySequence::Pages(pages)),
                                Ok(None) => Ok(AuxiliarySequence::None),
                                Err(error) => Err(error),
                            }
                        }
                        Err(error) => Err(error),
                    }
                });
            let sequence = sequence.map_err(|error| error.to_string());
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let details = source.as_ref().map_or_else(
                || crate::image_info::ImageDetails::load(&job_path),
                |source| {
                    crate::image_info::ImageDetails::load_from_source_while(
                        &job_path,
                        source,
                        || current_generation.load(Ordering::Acquire) == generation,
                    )
                },
            );
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let rating = source.as_ref().map_or(
                RatingObservation {
                    state: RatingState::Unreadable,
                    capability: RatingWriteCapability::UnsafeSource,
                },
                |source| {
                    crate::ratings::observe_source_while(source, &job_path, || {
                        current_generation.load(Ordering::Acquire) == generation
                    })
                },
            );
            if current_generation.load(Ordering::Acquire) != generation {
                return;
            }
            let _ = completion.complete((sequence, details, rating));
        });
        match scheduled {
            Ok(()) => self.auxiliary_job = Some(job),
            Err(error) => {
                log::error!("failed to queue current-image details");
                self.show_toast(format!("Image details unavailable: {error}"));
            }
        }
    }

    fn poll_auxiliary_load(&mut self) {
        let Some(job) = self.auxiliary_job.as_ref() else {
            return;
        };
        let polled = job.poll();
        if matches!(polled, JobPoll::Pending) {
            return;
        }
        let context = self
            .auxiliary_job
            .take()
            .expect("auxiliary job exists after polling it")
            .into_context();
        if !presented_work_is_current(
            context.generation,
            &context.path,
            self.session.generation.load(Ordering::Acquire),
            self.session.selected_path.as_deref(),
            self.session.presented_path.as_deref(),
        ) {
            return;
        }
        let (result, details, rating) = match polled {
            JobPoll::Ready(result) => result,
            JobPoll::Disconnected => {
                log::error!("current-image details worker disconnected");
                let rating = rating_after_auxiliary_disconnect();
                self.current_rating_capability = rating.capability;
                self.presented_rating = next_presented_rating(
                    self.presented_rating,
                    PresentedRatingTransition::Replace(rating.state),
                );
                if let Some(playlist) = self.playlist.as_mut() {
                    playlist.set_rating(&context.path, rating.state);
                }
                self.show_toast(auxiliary_disconnect_message());
                self.request_redraw();
                return;
            }
            JobPoll::Pending => unreachable!("pending auxiliary result returned early"),
        };
        let path = context.path;
        self.image_details = Some(details);
        self.current_rating_capability = rating.capability;
        self.presented_rating = next_presented_rating(
            self.presented_rating,
            PresentedRatingTransition::Replace(rating.state),
        );
        if let Some(playlist) = self.playlist.as_mut() {
            playlist.set_rating(&path, rating.state);
        }
        match result {
            Ok(AuxiliarySequence::Animation(animation)) => {
                let mut playback =
                    crate::animated::AnimationPlayback::new(animation, Instant::now());
                if self.transform.is_cropping || self.heal.active {
                    playback.pause();
                }
                let image = playback.current_image();
                if let Err(error) = self.present_sequence_image(&image, true) {
                    self.show_toast(format!(
                        "Animation unavailable; showing first frame: {error}"
                    ));
                    return;
                }
                self.animation = Some(playback);
            }
            Ok(AuxiliarySequence::Pages(pages)) => {
                let mut cursor = crate::pages::PageCursor::new(pages);
                if let Some(image) = self.current_image.as_ref() {
                    cursor.select_matching(image.width, image.height);
                }
                let image = cursor.current_image();
                let replace = self.current_image.as_ref().is_none_or(|current| {
                    current.width != image.width || current.height != image.height
                });
                if let Err(error) = self.present_sequence_image(&image, replace) {
                    self.show_toast(format!(
                        "Pages unavailable; showing the first image: {error}"
                    ));
                    return;
                }
                self.pages = Some(cursor);
            }
            Ok(AuxiliarySequence::None) => {}
            Err(error) => {
                log::debug!("container sequence unavailable");
                self.show_toast(format!(
                    "Container pages unavailable; showing the first image: {error}"
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
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
    }

    fn present_sequence_image(
        &mut self,
        image: &Arc<DecodedImage>,
        replace_displayed: bool,
    ) -> Result<(), String> {
        if !replace_displayed || self.transform.is_cropping || self.heal.active {
            return Ok(());
        }
        self.upload_realtime_image(image)?;
        self.current_image = Some(Arc::clone(image));
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
        Ok(())
    }

    fn step_sequence(&mut self, delta: isize) {
        if self.transform.is_cropping
            || self.heal.active
            || self.heal.history.can_undo()
            || self.unsaved_crop
        {
            if self.animation.is_some() || self.pages.is_some() {
                self.show_toast(crate::pages::edit_blocks_page_step_copy().to_owned());
            }
            return;
        }
        let previous_size = self
            .current_image
            .as_ref()
            .map(|image| (image.width, image.height));
        let image = if let Some(playback) = self.animation.as_mut() {
            if !playback.step(delta) {
                return;
            }
            playback.current_image()
        } else if let Some(cursor) = self.pages.as_mut() {
            if !cursor.step(delta) {
                return;
            }
            cursor.current_image()
        } else {
            return;
        };
        if let Err(error) = self.upload_realtime_image(&image) {
            self.animation = None;
            self.pages = None;
            self.show_toast(format!("Could not show that page: {error}"));
            return;
        }
        let size_changed = previous_size
            .is_some_and(|(width, height)| width != image.width || height != image.height);
        self.current_image = Some(image);
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
        if size_changed {
            self.fit_to_view();
        }
        self.request_redraw();
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
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
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
        self.current_preview = None;
        renderer.window().request_redraw();
        Ok(())
    }

    fn pause_animation(&mut self) {
        if let Some(playback) = self.animation.as_mut() {
            playback.pause();
        }
    }

    fn request_rating_assignment(&mut self, assignment: RatingAssignment) {
        if let Some(message) = rating_write_discovery_blocker(self.rating_scan_worker.is_some()) {
            self.show_toast(message);
            return;
        }
        if self.block_action_while_busy("changing the rating") {
            return;
        }
        if let Some(message) = rating_recovery_blocker(self.rating_recovery_unsettled) {
            self.show_toast(message);
            return;
        }
        let Some(path) = self.session.presented_path.clone() else {
            self.show_toast("Open an image before assigning a rating");
            return;
        };
        if self.session.selected_path.as_ref() != Some(&path) || self.current_source.is_none() {
            self.show_toast("Wait for the selected image to finish loading");
            return;
        }
        if assignment.expected_state() == self.presented_rating {
            return;
        }
        match self.current_rating_capability {
            RatingWriteCapability::WritableJpeg => {}
            RatingWriteCapability::ReadOnlyFormat => {
                self.show_toast(
                    "This image's rating is read-only in viewr. The file was not changed.",
                );
                return;
            }
            RatingWriteCapability::UnsupportedMetadata => {
                self.show_toast(
                    "This image has unsupported rating metadata. The file was not changed.",
                );
                return;
            }
            RatingWriteCapability::UnsafeSource => {
                self.show_toast(
                    "viewr could not verify this image's source safely. The file was not changed.",
                );
                return;
            }
            RatingWriteCapability::ObservationFailed => {
                self.show_toast(
                    "The rating could not be read. Close and reopen viewr before changing this file.",
                );
                return;
            }
        }
        let pending = PendingRatingWrite { path, assignment };
        if self.rating_write_disclosed {
            let _ = self.start_rating_write(&pending);
        } else {
            self.pending_rating_write = Some(pending);
            self.request_redraw();
        }
    }

    fn confirm_rating_disclosure(&mut self) {
        let Some(pending) = self.pending_rating_write.take() else {
            return;
        };
        if self.start_rating_write(&pending) {
            self.rating_write_disclosed = true;
        }
    }

    fn cancel_rating_disclosure(&mut self) {
        self.pending_rating_write = None;
        self.request_redraw();
    }

    fn start_rating_write(&mut self, pending: &PendingRatingWrite) -> bool {
        if let Some(message) = rating_write_discovery_blocker(self.rating_scan_worker.is_some()) {
            self.show_toast(message);
            return false;
        }
        if self.block_action_while_busy("changing the rating") {
            return false;
        }
        if let Some(message) = rating_recovery_blocker(self.rating_recovery_unsettled) {
            self.show_toast(message);
            return false;
        }
        if !rating_write_target_is_current(
            self.session.selected_path.as_ref() == Some(&pending.path),
            self.session.presented_path.as_ref() == Some(&pending.path),
        ) {
            self.show_toast("The selected image changed before the rating could be saved");
            return false;
        }
        let Some(source) = self.current_source.clone() else {
            self.show_toast("Wait for the selected image to finish loading");
            return false;
        };
        let path = pending.path.clone();
        let assignment = pending.assignment;
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let job_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name("viewr-rating-write".into())
            .spawn(move || {
                let result = crate::ratings::write_rating(&job_path, &source, assignment);
                let _ = sender.send(result);
                let _ = event_proxy.send_event(UserEvent::Wake);
            });
        if let Ok(join) = spawned {
            self.rating_write_worker = Some(RatingWriteWorker {
                path,
                assignment,
                result_rx: receiver,
                join,
            });
            self.show_toast("Saving rating...");
            true
        } else {
            self.show_toast("Could not save the rating safely. The previous rating is unchanged.");
            false
        }
    }

    fn poll_rating_write(&mut self, event_loop: &ActiveEventLoop) {
        let Some(worker) = self.rating_write_worker.as_ref() else {
            return;
        };
        let terminal = match poll_worker(&worker.result_rx) {
            WorkerPoll::Pending => return,
            WorkerPoll::Ready(result) => RatingWriteTerminal::Completed(result),
            WorkerPoll::Disconnected => RatingWriteTerminal::Disconnected,
        };
        let worker = self
            .rating_write_worker
            .take()
            .expect("rating worker exists after reaching terminal channel state");
        let worker_panicked = worker.join.join().is_err();
        if worker_panicked {
            log::error!("rating write worker panicked after terminal channel state");
        }
        let result = reconcile_rating_write(terminal, worker_panicked);
        let terminal_error = result.as_ref().err().copied();
        let close_disposition = rating_close_disposition(
            std::mem::take(&mut self.close_after_rating_write),
            terminal_error,
        );
        match result {
            Ok(verified) => {
                self.remove_prefetched_image(&worker.path);
                if let Some(playlist) = self.playlist.as_mut() {
                    playlist.set_rating(&worker.path, verified.state);
                    if let Some(provenance) = verified.source.scan_provenance() {
                        playlist.set_scan_provenance(&worker.path, Some(provenance));
                    }
                }
                if self.session.presented_path.as_ref() == Some(&worker.path) {
                    self.presented_rating = next_presented_rating(
                        self.presented_rating,
                        PresentedRatingTransition::Replace(verified.state),
                    );
                    self.current_source = Some(Arc::new(verified.source));
                    self.current_rating_capability = RatingWriteCapability::WritableJpeg;
                    self.start_coherence_watch();
                    self.start_auxiliary_load(&worker.path);
                }
                match worker.assignment {
                    RatingAssignment::Clear => self.show_toast("Rating cleared."),
                    RatingAssignment::Set(rating) => {
                        self.show_toast(format!("Rating {} of 5 saved.", rating.get()));
                    }
                }
            }
            Err(error) => {
                log::warn!("rating write failed: {error}");
                if error == RatingWriteError::RecoveryFailed {
                    self.rating_recovery_unsettled = next_rating_recovery_state(
                        self.rating_recovery_unsettled,
                        RatingRecoveryTransition::MarkUnsettled,
                    );
                    if self.session.presented_path.as_ref() == Some(&worker.path) {
                        self.current_source = None;
                        self.current_rating_capability = RatingWriteCapability::UnsafeSource;
                        self.current_image_reuse = ImageReuseEligibility::Ineligible;
                        self.presented_rating = next_presented_rating(
                            self.presented_rating,
                            PresentedRatingTransition::Replace(RatingState::Unreadable),
                        );
                        if let Some(playlist) = self.playlist.as_mut() {
                            playlist.set_rating(&worker.path, RatingState::Unreadable);
                        }
                    }
                }
                self.show_toast(rating_write_failure_message(error));
            }
        }
        self.kick_prefetch();
        self.request_redraw();
        if matches!(close_disposition, RatingCloseDisposition::Exit) {
            event_loop.exit();
        }
    }

    fn set_rating_filter(&mut self, filter: RatingFilter) {
        if self.block_action_while_busy("changing the rating filter") {
            return;
        }
        let worker_active = self.rating_scan_worker.is_some();
        let selection = if let Some(playlist) = self.playlist.as_mut() {
            if filter == RatingFilter::All {
                playlist.show_all()
            } else {
                playlist.set_filter(filter)
            }
        } else {
            return;
        };
        let has_loading_ratings = self
            .playlist
            .as_ref()
            .is_some_and(Playlist::has_loading_ratings);
        match rating_discovery_transition(filter, worker_active, has_loading_ratings) {
            RatingDiscoveryTransition::Apply => {}
            RatingDiscoveryTransition::Start => {
                self.start_rating_discovery();
                self.request_redraw();
                return;
            }
            RatingDiscoveryTransition::KeepRunning => {
                self.request_redraw();
                return;
            }
            RatingDiscoveryTransition::CancelAndApply => {
                if let Some(worker) = self.rating_scan_worker.take() {
                    worker.cancel.store(true, Ordering::Release);
                }
            }
        }
        self.apply_filter_selection(selection);
    }

    fn start_rating_discovery(&mut self) {
        let Some(playlist) = self.playlist.as_ref() else {
            return;
        };
        let files = playlist
            .files_with_provenance()
            .map(|(path, provenance)| (path.to_owned(), provenance))
            .collect::<Vec<_>>();
        let generation = self.rating_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::channel();
        let event_proxy = self.event_proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("viewr-rating-scan".into())
            .spawn(move || {
                let ratings = crate::ratings::scan_folder_ratings_while(
                    files,
                    || !worker_cancel.load(Ordering::Acquire),
                    crate::decode::max_concurrent_file_decodes(),
                );
                if let Some(ratings) = ratings {
                    let _ = sender.send(ratings);
                    let _ = event_proxy.send_event(UserEvent::Wake);
                }
            });
        if spawned.is_ok() {
            self.rating_scan_worker = Some(RatingScanWorker {
                generation,
                cancel,
                result_rx: receiver,
            });
        } else {
            if let Some(playlist) = self.playlist.as_mut() {
                playlist.show_all();
            }
            self.show_toast("Could not finish reading folder ratings. Showing all images.");
        }
    }

    fn poll_rating_discovery(&mut self) {
        let Some(worker) = self.rating_scan_worker.as_ref() else {
            return;
        };
        let completed = match worker.result_rx.try_recv() {
            Ok(ratings) => Some(Ok(ratings)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
        };
        let Some(completed) = completed else {
            return;
        };
        let worker = self
            .rating_scan_worker
            .take()
            .expect("rating scan exists after polling it");
        if worker.generation != self.rating_generation {
            return;
        }
        let Ok(ratings) = completed else {
            if let Some(playlist) = self.playlist.as_mut() {
                playlist.show_all();
            }
            self.show_toast("Could not finish reading folder ratings. Showing all images.");
            return;
        };
        let selection = if let Some(playlist) = self.playlist.as_mut() {
            let filter = playlist.filter();
            playlist.set_discovered_ratings(&ratings);
            if let Some(path) = self.session.presented_path.as_deref()
                && let Some(rating) = playlist.rating_for_known_path(path)
            {
                self.presented_rating = next_presented_rating(
                    self.presented_rating,
                    PresentedRatingTransition::Replace(rating),
                );
            }
            playlist.set_filter(filter)
        } else {
            return;
        };
        self.apply_filter_selection(selection);
    }

    fn apply_filter_selection(&mut self, selection: FilterSelection) {
        if filter_selection_changes_source(selection, self.current_image.is_some()) {
            self.cancel_save_overwrite_for_source_change();
            self.cancel_rating_disclosure_for_source_change();
        }
        self.reset_prefetch_for_playlist_change();
        match selection {
            FilterSelection::Stay => {
                if self.current_image.is_none()
                    && let Some(path) = self
                        .playlist
                        .as_ref()
                        .and_then(|playlist| playlist.files.get(playlist.index))
                        .cloned()
                {
                    self.session.selected_path = Some(path.clone());
                    self.spawn_image_load(path);
                }
            }
            FilterSelection::Select(index) => self.go_to_index(index),
            FilterSelection::Empty => {
                self.cancel_pending_image_load();
                self.session.selected_path = None;
                self.invalidate_displayed_image();
            }
        }
        self.kick_prefetch();
        self.request_redraw();
    }

    fn discard_animation_for_pixel_edit(&mut self) {
        self.animation = None;
        self.pages = None;
        self.auxiliary_job = None;
    }

    fn cancel_pending_image_load(&mut self) {
        self.session.cancel_pending_load();
        self.preview_job = None;
        self.preview_load_retry_blocked = false;
    }

    fn retry_current_image_load(&mut self) {
        if let Some(message) = preview_retry_blocker(self.preview_load_retry_blocked) {
            self.show_toast(message);
            return;
        }
        if self.block_action_while_curating("retrying the image load") {
            return;
        }
        let Some(path) = self.session.selected_path.clone() else {
            return;
        };
        match foreground_retry_plan(self.session.selected_missing) {
            ForegroundRetryPlan::LoadSelected => self.spawn_image_load(path),
            ForegroundRetryPlan::LoadAndScanFolder => self.load_and_scan(path),
        }
        self.request_redraw();
    }

    fn reload_current_image(&mut self) {
        use crate::file_coherence::ReloadStartBlocker;

        if self.block_action_while_curating("reloading this file") {
            return;
        }
        if let Some(blocker) = crate::file_coherence::reload_start_blocker([
            (self.heal.is_busy() || self.heal.painting).then_some(ReloadStartBlocker::SpotHeal),
            self.crop_job.is_some().then_some(ReloadStartBlocker::Crop),
            self.save_transaction_active()
                .then_some(ReloadStartBlocker::Save),
            self.rating_write_worker
                .is_some()
                .then_some(ReloadStartBlocker::RatingWrite),
            self.rating_scan_worker
                .is_some()
                .then_some(ReloadStartBlocker::RatingDiscovery),
            (self.session.is_loading() || self.preview_job.is_some())
                .then_some(ReloadStartBlocker::ImagePreparation),
        ]) {
            self.show_toast(crate::file_coherence::reload_start_blocker_message(blocker));
            return;
        }
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        self.cancel_open_with_check();

        // A reload is an explicit disk refresh. Drop any speculative copy,
        // invalidate older speculative work, and remove the stale filmstrip
        // texture. Keep the last good pixels presented while the replacement
        // decodes in the foreground.
        self.remove_prefetched_image(&path);
        self.prefetch_schedule.reset();
        self.thumbnail_schedule.reset();
        self.thumb_textures.remove(&path);
        self.transform = Transform::default();
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
        self.spawn_refreshed_image_load(path);
        self.show_toast("Reloading file from disk");
        self.request_redraw();
    }

    fn open_current_with(&mut self) {
        if self.block_action_while_busy("opening the source in another app") {
            return;
        }
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            self.show_toast("Open With requires the current image to finish loading");
            return;
        };
        let Some(source) = self.current_source.as_ref().map(Arc::clone) else {
            self.show_toast("Could not verify the current source for Open With");
            return;
        };
        let generation = self.session.generation.load(Ordering::Acquire);
        let current_generation = Arc::clone(&self.session.generation);
        let cancel = Arc::new(AtomicBool::new(false));
        let context = OpenWithContext {
            path: path.clone(),
            generation,
            cancel: Arc::clone(&cancel),
        };
        let event_proxy = self.event_proxy.clone();
        let (completion, job) = OneShotJob::new(context, move || {
            let _ = event_proxy.send_event(UserEvent::Wake);
        });
        let spawn = std::thread::Builder::new()
            .name("viewr-open-with-check".into())
            .spawn(move || {
                let source_match = source.matches_path_while(&path, || {
                    !cancel.load(Ordering::Acquire)
                        && current_generation.load(Ordering::Acquire) == generation
                });
                let _ = completion.complete(source_match);
            });
        if spawn.is_err() {
            self.show_toast("Could not start source verification for Open With");
            return;
        }
        self.open_with_job = Some(job);
        self.show_toast("Verifying source for Open With");
    }

    fn cancel_open_with_check(&mut self) {
        if let Some(job) = self.open_with_job.take() {
            job.context().cancel.store(true, Ordering::Release);
        }
    }

    fn finish_open_with_check(&mut self) {
        let Some(job) = self.open_with_job.as_ref() else {
            return;
        };
        let polled = job.poll();
        if matches!(polled, JobPoll::Pending) {
            return;
        }
        let context = self
            .open_with_job
            .take()
            .expect("Open With job exists after terminal poll")
            .into_context();
        if !loaded_work_is_current(
            context.generation,
            &context.path,
            self.session.generation.load(Ordering::Acquire),
            self.current_loaded_path(),
        ) {
            return;
        }
        let source_match = match polled {
            JobPoll::Ready(source_match) => source_match,
            JobPoll::Disconnected => {
                self.show_toast("Could not finish source verification for Open With");
                return;
            }
            JobPoll::Pending => unreachable!("pending Open With check returned early"),
        };
        match source_match {
            crate::fs::ImageSourceMatch::Same => {}
            crate::fs::ImageSourceMatch::Changed | crate::fs::ImageSourceMatch::Missing => {
                self.show_toast("Source changed on disk. Press F5 before Open With");
                return;
            }
            crate::fs::ImageSourceMatch::Unsupported => {
                self.show_toast("Open With is unavailable for this linked or unsupported source");
                return;
            }
            crate::fs::ImageSourceMatch::Unavailable => {
                self.show_toast("Could not verify the current source for Open With");
                return;
            }
        }
        self.show_open_with_dialog(&context.path);
    }

    fn show_open_with_dialog(&mut self, path: &Path) {
        self.context_menu_pos = None;
        let outcome = {
            #[cfg(target_os = "windows")]
            {
                use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
                let parent = self
                    .renderer
                    .as_ref()
                    .and_then(|renderer| renderer.window().window_handle().ok())
                    .and_then(|handle| match handle.as_raw() {
                        RawWindowHandle::Win32(handle) => {
                            Some(handle.hwnd.get() as windows_sys::Win32::Foundation::HWND)
                        }
                        _ => None,
                    })
                    .unwrap_or(std::ptr::null_mut());
                crate::open_with::show_open_with_dialog(path, parent)
            }
            #[cfg(not(target_os = "windows"))]
            {
                crate::open_with::show_open_with_dialog(path)
            }
        };
        match outcome {
            crate::open_with::OpenWithOutcome::Launched => {
                self.external_edit_pending = true;
                self.show_toast("Source opened in another app. Changes reload when that is safe");
            }
            crate::open_with::OpenWithOutcome::Cancelled => self.show_toast("Open With canceled"),
            crate::open_with::OpenWithOutcome::InvalidPath => {
                log::error!("Open With rejected an invalid path");
                self.show_toast("Could not open the app chooser");
            }
            crate::open_with::OpenWithOutcome::Failed => {
                log::error!("Open With chooser failed");
                self.show_toast("Could not open the app chooser");
            }
        }
    }

    fn start_coherence_watch(&mut self) {
        self.stop_coherence_watch();
        self.last_coherence_action = None;
        if !crate::file_coherence::watch_can_start(
            self.current_image.is_some(),
            self.current_source.is_some(),
            self.session.presented_path.as_deref() == self.session.selected_path.as_deref()
                && self.session.selected_path.is_some(),
        ) {
            return;
        }
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        let Some(source) = self.current_source.clone() else {
            return;
        };
        let folder = path.parent().map(Path::to_owned);
        let folder_stamp = folder.as_deref().and_then(crate::fs::directory_stamp);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        let event_proxy = self.event_proxy.clone();
        let watch_path = path.clone();
        let spawn = std::thread::Builder::new()
            .name("viewr-file-coherence".into())
            .spawn(move || {
                run_coherence_watch(
                    path,
                    source,
                    folder,
                    folder_stamp,
                    worker_cancel,
                    worker_latest,
                    event_proxy,
                );
            });
        if spawn.is_ok() {
            self.coherence_watch = Some(CoherenceWatch {
                path: watch_path,
                cancel,
                latest,
            });
        }
    }

    fn stop_coherence_watch(&mut self) {
        if let Some(watch) = self.coherence_watch.take() {
            watch.cancel.store(true, Ordering::Release);
        }
    }

    fn poll_coherence_watch(&mut self) {
        let Some(watch) = self.coherence_watch.as_ref() else {
            return;
        };
        let watch_path = watch.path.clone();
        let observation = watch
            .latest
            .lock()
            .ok()
            .and_then(|mut latest| latest.take());
        let Some(observation) = observation else {
            return;
        };
        if !crate::file_coherence::watch_applies(self.current_loaded_path(), &watch_path) {
            return;
        }
        let facts = crate::file_coherence::CoherenceFacts::from_observation(
            observation,
            crate::file_coherence::UnsavedEdits {
                cropping: self.transform.is_cropping || self.crop_job.is_some(),
                applied_crop: self.unsaved_crop,
                heal_pending: self.heal.active
                    || self.heal.is_busy()
                    || self.heal.painting
                    || self.heal.history.can_undo(),
                rotated_or_flipped: self.transform.rotation_steps != 0
                    || self.transform.flip_h
                    || self.transform.flip_v,
            },
            crate::file_coherence::SessionBusy {
                loading: self.session.is_loading() || self.preview_job.is_some(),
                saving: self.save_transaction_active(),
                healing: self.heal.is_busy() || self.heal.painting,
                cropping: self.crop_job.is_some(),
                curating: self.curation_worker.is_some(),
                rating: self.rating_write_worker.is_some() || self.rating_scan_worker.is_some(),
            },
        );
        self.apply_coherence_action(crate::file_coherence::coalesce(
            crate::file_coherence::CoherenceAction::Ignore,
            crate::file_coherence::decide(facts),
        ));
    }

    fn apply_coherence_action(&mut self, action: crate::file_coherence::CoherenceAction) {
        use crate::file_coherence::CoherenceAction;
        let announce = crate::file_coherence::should_announce(self.last_coherence_action, action);
        if !matches!(action, CoherenceAction::Ignore) {
            self.last_coherence_action = Some(action);
        }
        match action {
            CoherenceAction::Ignore => {}
            CoherenceAction::RemindReload => {
                self.external_edit_pending = true;
                if announce {
                    self.show_toast(crate::file_coherence::reload_reminder_copy());
                }
            }
            CoherenceAction::ReloadCurrent => self.reload_current_from_disk_quietly(),
            CoherenceAction::ReloadAndRescan => {
                self.reload_current_from_disk_quietly();
                self.refresh_folder_membership();
            }
            CoherenceAction::CurrentGone => {
                self.cancel_rating_disclosure_for_source_change();
                self.external_edit_pending = false;
                self.source_gone = true;
                if announce {
                    self.show_toast(crate::file_coherence::current_gone_copy());
                }
            }
            CoherenceAction::RescanFolder => self.refresh_folder_membership(),
            CoherenceAction::RemindAndRescan => {
                self.cancel_rating_disclosure_for_source_change();
                self.external_edit_pending = true;
                if announce {
                    self.show_toast(crate::file_coherence::reload_reminder_copy());
                }
                self.refresh_folder_membership();
            }
            CoherenceAction::GoneAndRescan => {
                self.cancel_rating_disclosure_for_source_change();
                self.external_edit_pending = false;
                self.pending_gone_notice = true;
                self.refresh_folder_membership();
            }
        }
    }

    fn reload_current_from_disk_quietly(&mut self) {
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        self.remove_prefetched_image(&path);
        self.prefetch_schedule.reset();
        self.thumb_textures.remove(&path);
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
        self.spawn_refreshed_image_load(path);
    }

    fn settle_pending_gone_notice(&mut self, found_same_path: bool, found_rename: bool) {
        if !self.pending_gone_notice {
            return;
        }
        self.pending_gone_notice = false;
        match crate::file_coherence::gone_rescan_result(found_same_path, found_rename) {
            crate::file_coherence::GoneRescanResult::Reappeared => {
                self.source_gone = false;
                if !self.session.is_loading() && self.preview_job.is_none() {
                    self.reload_current_from_disk_quietly();
                }
            }
            crate::file_coherence::GoneRescanResult::Renamed => {
                self.source_gone = false;
                self.show_toast(crate::file_coherence::renamed_copy());
            }
            crate::file_coherence::GoneRescanResult::Missing => {
                self.source_gone = true;
                self.show_toast(crate::file_coherence::current_gone_copy());
            }
        }
    }

    fn refresh_folder_membership(&mut self) {
        if self.folder_scan_job.is_some() {
            return;
        }
        let Some(path) = self.session.selected_path.clone() else {
            return;
        };
        let Some(directory) = path.parent().map(Path::to_owned) else {
            return;
        };
        self.start_folder_scan(
            directory,
            ScanPurpose::SelectedFile {
                path,
                missing_recovery: false,
            },
        );
    }

    fn current_loaded_path(&self) -> Option<&Path> {
        let path = self.session.selected_path.as_deref()?;
        (self.current_image.is_some() && self.session.presented_path.as_deref() == Some(path))
            .then_some(path)
    }

    fn dock_input(&self) -> crate::chrome::DockInput {
        let has_filmstrip = self
            .playlist
            .as_ref()
            .is_some_and(|playlist| filmstrip_is_available(playlist.visible_len()));
        let has_image = self
            .renderer
            .as_ref()
            .and_then(Renderer::image_size)
            .is_some();
        crate::chrome::DockInput {
            has_image,
            has_multiple_images: has_filmstrip,
            show_tools: self.show_tools_panel,
            tools_expanded: self.tools_panel_open,
            tools_side: self.tools_panel_side,
            show_filmstrip: self.show_filmstrip_panel,
            filmstrip_expanded: self.filmstrip_panel_open,
            show_image_info: self.show_image_info,
            image_info_side: self.image_info_side,
            heal_active: self.heal.active,
            immersive: self.is_fullscreen || self.mosaic.is_active(),
        }
    }

    fn viewport_insets(&self) -> crate::view::ViewportInsets {
        let scale_factor = self
            .renderer
            .as_ref()
            .map_or(1.0, |renderer| renderer.window().scale_factor());
        crate::chrome::viewport_insets(
            crate::chrome::DockViewModel::new(self.dock_input()).layout(scale_factor),
        )
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
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        self.is_fullscreen = !self.is_fullscreen;
        if self.is_fullscreen {
            renderer
                .window()
                .set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        } else {
            renderer.window().set_fullscreen(None);
        }
        self.observe_current_display();
        self.request_redraw();
    }

    fn toggle_full_image_mosaic(&mut self) {
        if self.mosaic.is_active() {
            self.leave_full_image_mosaic();
            return;
        }
        if self.block_browse_while_busy() {
            return;
        }
        let Some(playlist) = self.playlist.as_ref() else {
            self.show_toast("Full-image collage needs an open folder");
            return;
        };
        if playlist.visible_len() < 2 {
            self.show_toast("Full-image collage needs more than one matching photo");
            return;
        }
        let Some(current_index) = playlist.catalog_index() else {
            self.show_toast("Full-image collage needs a selected photo");
            return;
        };
        let Some(page) = crate::mosaic::MosaicPage::containing(
            playlist.visible_projection(),
            current_index,
            crate::mosaic::MAX_IMAGES,
        ) else {
            return;
        };
        self.install_mosaic_page(page);
    }

    fn install_mosaic_page(&mut self, page: crate::mosaic::MosaicPage) {
        let Some(playlist) = self.playlist.as_ref() else {
            return;
        };
        let retained_paths: HashSet<PathBuf> = page
            .indices
            .iter()
            .filter_map(|index| playlist.files.get(*index).cloned())
            .collect();
        let current_bytes = self
            .current_image
            .as_ref()
            .map_or(0, |image| image.rgba.len());
        let neighbor_budget = prefetch::DEFAULT_MAX_BYTES.saturating_sub(current_bytes);

        self.prefetch_schedule.reset();
        self.prefetch.retain(|path| retained_paths.contains(path));
        self.prefetch
            .set_limits(crate::mosaic::MAX_IMAGES, neighbor_budget);
        let prefetch = &self.prefetch;
        self.prefetch_sources
            .retain(|path, _| prefetch.contains(path));
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_mosaic_slot_count(page.indices.len());
        }
        self.mosaic = MosaicView {
            uploaded_paths: vec![None; page.indices.len()],
            page: Some(page),
            memory_limited: current_bytes >= prefetch::DEFAULT_MAX_BYTES,
            display_limited: false,
            unavailable_paths: HashSet::new(),
        };
        self.sync_mosaic_gpu();
        self.kick_prefetch();
        self.request_redraw();
    }

    fn leave_full_image_mosaic(&mut self) {
        if !self.mosaic.is_active() {
            return;
        }
        self.clear_full_image_mosaic();
        self.kick_prefetch();
    }

    fn clear_full_image_mosaic(&mut self) {
        self.prefetch_schedule.reset();
        self.prefetch
            .set_limits(prefetch::DEFAULT_CAPACITY, prefetch::DEFAULT_MAX_BYTES);
        self.mosaic = MosaicView::default();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_mosaic_images();
        }
        self.request_redraw();
    }

    fn sync_mosaic_gpu(&mut self) {
        let Some(page) = self.mosaic.page.as_ref() else {
            return;
        };
        let Some(playlist) = self.playlist.as_ref() else {
            return;
        };
        let candidates: Vec<(PathBuf, Option<Arc<DecodedImage>>, bool)> = page
            .indices
            .iter()
            .filter_map(|index| {
                let path = playlist.files.get(*index)?.clone();
                let is_current = self.session.presented_path.as_deref() == Some(path.as_path());
                let image = if is_current {
                    self.current_image.clone()
                } else {
                    self.prefetch.get_shared(&path)
                };
                Some((path, image, is_current))
            })
            .collect();
        let focused_path = page
            .indices
            .get(page.focused)
            .and_then(|index| playlist.files.get(*index))
            .cloned();
        let mut failed = Vec::new();
        let mut upload_count = 0_usize;
        let mut upload_pending = false;
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        for (slot, (path, image, is_current)) in candidates.into_iter().enumerate() {
            let already_uploaded = self
                .mosaic
                .uploaded_paths
                .get(slot)
                .and_then(Option::as_ref)
                == Some(&path);
            if already_uploaded {
                continue;
            }
            let Some(image) = image else {
                renderer.clear_mosaic_image(slot);
                if let Some(uploaded) = self.mosaic.uploaded_paths.get_mut(slot) {
                    *uploaded = None;
                }
                continue;
            };
            if is_current {
                match renderer.use_current_image_in_mosaic(slot) {
                    Ok(()) => {
                        if let Some(uploaded) = self.mosaic.uploaded_paths.get_mut(slot) {
                            *uploaded = Some(path);
                        }
                    }
                    Err(error) => {
                        log::warn!("full-image mosaic could not reuse current image: {error}");
                        failed.push(path);
                    }
                }
                continue;
            }
            if upload_count >= 1 {
                upload_pending = true;
                continue;
            }
            match renderer.set_mosaic_image(slot, &image, None) {
                Ok(_) => {
                    upload_count += 1;
                    if let Some(uploaded) = self.mosaic.uploaded_paths.get_mut(slot) {
                        *uploaded = Some(path);
                    }
                }
                Err(error) => {
                    log::warn!(
                        "full-image mosaic omitted {}: {error}",
                        prefetch::privacy_safe_file_name(&path)
                    );
                    renderer.clear_mosaic_image(slot);
                    if let Some(uploaded) = self.mosaic.uploaded_paths.get_mut(slot) {
                        *uploaded = None;
                    }
                    failed.push(path);
                }
            }
        }
        for path in failed {
            self.mosaic.display_limited = true;
            self.mosaic.unavailable_paths.insert(path.clone());
            self.remove_prefetched_image(&path);
        }
        self.recover_unavailable_mosaic_focus(focused_path.as_deref());
        if upload_pending {
            self.request_redraw();
        }
    }

    fn recover_unavailable_mosaic_focus(&mut self, focused_path: Option<&Path>) {
        let Some(page) = self.mosaic.page.as_mut() else {
            return;
        };
        if focused_path.is_some_and(|path| self.mosaic.unavailable_paths.contains(path))
            && let Some(first_loaded) = self.mosaic.uploaded_paths.iter().position(Option::is_some)
        {
            page.focused = first_loaded;
        }
    }

    fn move_mosaic_focus(&mut self, direction: crate::mosaic::FocusDirection) {
        let Some(page) = self.mosaic.page.as_mut() else {
            return;
        };
        let loaded_slots: Vec<usize> = self
            .mosaic
            .uploaded_paths
            .iter()
            .enumerate()
            .filter_map(|(slot, path)| path.as_ref().map(|_| slot))
            .collect();
        if loaded_slots.is_empty() {
            return;
        }
        let focused = loaded_slots
            .iter()
            .position(|slot| *slot == page.focused)
            .unwrap_or(0);
        let mut navigation = crate::mosaic::MosaicPage {
            start: 0,
            indices: loaded_slots.clone(),
            focused,
        };
        navigation.move_focus(direction);
        page.focused = loaded_slots[navigation.focused];
        self.request_redraw();
    }

    fn move_mosaic_page(&mut self, delta: isize) {
        let Some(current) = self.mosaic.page.as_ref() else {
            return;
        };
        let Some(playlist) = self.playlist.as_ref() else {
            return;
        };
        let Some(next) = current.adjacent(playlist.visible_projection(), delta) else {
            return;
        };
        self.install_mosaic_page(next);
    }

    fn open_focused_mosaic_photo(&mut self) {
        let Some(page) = self.mosaic.page.as_ref() else {
            return;
        };
        if self
            .mosaic
            .uploaded_paths
            .get(page.focused)
            .and_then(Option::as_ref)
            .is_none()
        {
            return;
        }
        let Some(index) = page.indices.get(page.focused).copied() else {
            return;
        };
        self.open_mosaic_photo(index);
    }

    fn open_mosaic_photo(&mut self, index: usize) {
        let Some(path) = self.mosaic.page.as_ref().and_then(|page| {
            page.indices
                .iter()
                .position(|candidate| *candidate == index)
                .filter(|slot| {
                    self.mosaic
                        .uploaded_paths
                        .get(*slot)
                        .and_then(Option::as_ref)
                        .is_some()
                })
                .and_then(|_| self.playlist.as_ref()?.files.get(index).cloned())
        }) else {
            return;
        };
        let reserved = (self.session.presented_path.as_deref() != Some(path.as_path()))
            .then(|| self.take_prefetched_image(&path))
            .flatten();
        self.clear_full_image_mosaic();
        if let Some(loaded) = reserved
            && !self.insert_prefetched_image(path.clone(), loaded)
        {
            log::warn!(
                "full-image mosaic selection could not retain {} for navigation",
                prefetch::privacy_safe_file_name(&path)
            );
        }
        self.go_to_index(index);
    }

    fn observe_current_display(&mut self) {
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        let current = renderer.window().current_monitor().and_then(|monitor| {
            crate::display_state::monitor_identity(
                monitor.name().as_deref(),
                (monitor.position().x, monitor.position().y),
                (monitor.size().width, monitor.size().height),
                monitor.scale_factor(),
            )
        });
        let observation =
            crate::display_state::observe_display(self.display_monitor.as_ref(), current.as_ref());
        if !crate::display_state::should_refresh_profile(observation) {
            return;
        }
        self.display_monitor = current;
        self.display_hints = crate::display_probe::refresh_display_hints(
            self.display_hints,
            self.display_monitor.as_ref(),
        );
        let output = if crate::display_state::should_fetch_profile(
            self.display_hints,
            self.display_monitor.as_ref(),
        ) {
            crate::display_probe::fetch_display_profile_bytes(
                self.display_monitor
                    .as_ref()
                    .and_then(crate::display_state::MonitorIdentity::name),
                self.renderer
                    .as_ref()
                    .map(|renderer| renderer.window().as_ref()),
            )
            .and_then(|bytes| {
                crate::display_output::DisplayOutputNormalizer::from_profile_bytes(&bytes)
            })
            .unwrap_or_else(crate::display_output::DisplayOutputNormalizer::identity)
        } else {
            crate::display_output::DisplayOutputNormalizer::identity()
        };
        self.display_profile_usable = output.is_applied();
        let had_image = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.image_size().is_some());
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_display_output(output);
            if self.mosaic.is_active() {
                renderer.set_mosaic_slot_count(self.mosaic.uploaded_paths.len());
                self.mosaic.uploaded_paths.fill(None);
            }
        }
        if had_image {
            self.refresh_presented_display_pixels();
        }
        if self.mosaic.is_active() {
            self.sync_mosaic_gpu();
        }
        self.request_redraw();
    }

    fn refresh_presented_display_pixels(&mut self) {
        let Some(image) = self.current_image.clone() else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let preview = self.current_preview.as_ref();
        if renderer.required_preview(&image).is_some() && preview.is_none() {
            return;
        }
        if let Err(error) = renderer.set_image(&image, preview) {
            log::error!("failed to refresh display color: {error}");
            self.show_toast(format!("Could not update display color: {error}"));
        }
    }

    fn request_redraw(&self) {
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn mosaic_frame_geometry(
        &mut self,
        scale_factor: f64,
    ) -> (
        Vec<crate::gpu::MosaicDraw>,
        Option<crate::ui::MosaicUiState>,
    ) {
        let Some(page) = self.mosaic.page.clone() else {
            return (Vec::new(), None);
        };
        let Some(renderer) = self.renderer.as_ref() else {
            return (Vec::new(), None);
        };
        let size = renderer.window().inner_size();
        let Some(viewport) = crate::view::safe_viewport_rect(
            (size.width, size.height),
            crate::view::ViewportInsets::default(),
        ) else {
            return (Vec::new(), None);
        };
        let loaded: Vec<(usize, (u32, u32))> = self
            .mosaic
            .uploaded_paths
            .iter()
            .enumerate()
            .filter_map(|(slot, path)| {
                path.as_ref()?;
                Some((slot, renderer.mosaic_image_size(slot)?))
            })
            .collect();
        let upload_ready = self.playlist.as_ref().is_some_and(|playlist| {
            page.indices.iter().enumerate().any(|(slot, index)| {
                self.mosaic
                    .uploaded_paths
                    .get(slot)
                    .and_then(Option::as_ref)
                    .is_none()
                    && playlist.files.get(*index).is_some_and(|path| {
                        self.session.presented_path.as_deref() == Some(path.as_path())
                            || self.prefetch.contains(path)
                    })
            })
        });
        let loading = self.prefetch_schedule.in_flight_len() > 0 || upload_ready;
        let gap = LogicalSize::new(3_u32, 3_u32)
            .to_physical::<u32>(scale_factor)
            .width
            .clamp(1, 12);
        let display_sizes = loaded
            .iter()
            .map(|(slot, image_size)| {
                page.indices
                    .get(*slot)
                    .map_or(*image_size, |catalog_index| {
                        self.mosaic_photo_display_size(*image_size, *catalog_index)
                    })
            })
            .collect::<Vec<_>>();
        let grid = crate::mosaic::dense_collage(viewport, &display_sizes, gap);
        let projection_total = self.playlist.as_ref().map_or(0, Playlist::visible_len);
        let mut draws = Vec::with_capacity(loaded.len());
        let mut cells = Vec::with_capacity(loaded.len());
        for ((slot, image_size), cell) in loaded.into_iter().zip(grid.cells) {
            let Some(catalog_index) = page.indices.get(slot).copied() else {
                continue;
            };
            let placement = self.mosaic_photo_placement(
                (size.width, size.height),
                image_size,
                cell,
                catalog_index,
            );
            draws.push(crate::gpu::MosaicDraw {
                slot,
                placement,
                viewport: cell,
            });
            if let Some(rect) = cell.logical_bounds(scale_factor) {
                cells.push(crate::ui::MosaicCell {
                    catalog_index,
                    projection_position: page.start.saturating_add(slot).saturating_add(1),
                    projection_total,
                    rect,
                    selected: page.focused == slot,
                });
            }
        }
        let ready = draws.len();
        let target = page.indices.len();
        let state = self.mosaic_load_state(ready, target, loading);
        (
            draws,
            Some(crate::ui::MosaicUiState {
                cells,
                ready,
                target,
                state,
            }),
        )
    }

    fn mosaic_photo_display_size(
        &self,
        image_size: (u32, u32),
        catalog_index: usize,
    ) -> (u32, u32) {
        let is_current = self.playlist.as_ref().is_some_and(|playlist| {
            playlist.index == catalog_index
                && self.session.presented_path.as_deref()
                    == playlist.files.get(catalog_index).map(PathBuf::as_path)
        });
        if is_current && self.transform.rotation_steps.rem_euclid(2) != 0 {
            (image_size.1, image_size.0)
        } else {
            image_size
        }
    }

    fn mosaic_load_state(
        &self,
        ready: usize,
        target: usize,
        loading: bool,
    ) -> crate::ui::MosaicLoadState {
        if loading {
            crate::ui::MosaicLoadState::Loading
        } else if ready == target {
            crate::ui::MosaicLoadState::Ready
        } else if self.mosaic.memory_limited {
            crate::ui::MosaicLoadState::MemoryLimited
        } else if self.mosaic.display_limited {
            crate::ui::MosaicLoadState::DisplayLimited
        } else {
            crate::ui::MosaicLoadState::Incomplete
        }
    }

    fn mosaic_photo_placement(
        &self,
        target: (u32, u32),
        image_size: (u32, u32),
        cell: crate::view::PhysicalViewport,
        catalog_index: usize,
    ) -> crate::view::Placement {
        let is_current = self.playlist.as_ref().is_some_and(|playlist| {
            playlist.index == catalog_index
                && self.session.presented_path.as_deref()
                    == playlist.files.get(catalog_index).map(PathBuf::as_path)
        });
        let rotated90 = is_current && self.transform.rotation_steps.rem_euclid(2) != 0;
        let mut placement =
            crate::view::fit_to_physical_viewport(target, image_size, cell, rotated90);
        if is_current {
            placement.uv_matrix = crate::view::uv_transform(
                self.transform.rotation_steps,
                self.transform.flip_h,
                self.transform.flip_v,
            );
        }
        placement
    }

    fn rotate_current(&mut self, quarter_turns: i32) {
        if self.block_action_with_mode_allowance("rotating the image", ActiveModeAllowance::Crop) {
            return;
        }
        if self.current_loaded_path().is_some() {
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
        if self.block_action_with_mode_allowance("flipping the image", ActiveModeAllowance::Crop) {
            return;
        }
        if self.current_loaded_path().is_some() {
            self.transform.flip_h = !self.transform.flip_h;
            self.request_redraw();
        }
    }

    fn flip_current_vertically(&mut self) {
        if self.block_action_with_mode_allowance("flipping the image", ActiveModeAllowance::Crop) {
            return;
        }
        if self.current_loaded_path().is_some() {
            self.transform.flip_v = !self.transform.flip_v;
            self.request_redraw();
        }
    }

    fn handle_single_key_shortcut(&mut self, key: &str) {
        if let Some(assignment) = rating_assignment_for_key(key, false) {
            if rating_keys_apply(self.transform.is_cropping, self.heal.active) {
                self.request_rating_assignment(assignment);
            }
            return;
        }
        match key {
            "o" | "O" => self.open_image_dialog(),
            "t" | "T" => {
                self.show_tools_panel = !self.show_tools_panel;
                self.request_redraw();
            }
            "g" | "G" if self.modifiers.shift_key() => self.toggle_full_image_mosaic(),
            "g" | "G" => {
                if self
                    .playlist
                    .as_ref()
                    .is_some_and(|playlist| filmstrip_is_available(playlist.visible_len()))
                {
                    self.show_filmstrip_panel = !self.show_filmstrip_panel;
                    self.request_redraw();
                } else {
                    self.show_toast("Folder previews need more than one image");
                }
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
            "u" | "U" if !self.heal.active => self.undo_trash(),
            "x" | "X" if self.transform.is_cropping => self.swap_crop_ratio(),
            "+" | "=" => self.zoom_at_viewport_center(1.15),
            "-" | "_" => self.zoom_at_viewport_center(1.0 / 1.15),
            "[" => self.step_sequence(-1),
            "]" => self.step_sequence(1),
            _ => {}
        }
    }

    fn navigate(&mut self, delta: isize) {
        if let Some(playlist) = &self.playlist
            && let Some(new_index) = playlist.navigation_target(delta)
            && new_index != playlist.index
        {
            self.go_to_index(new_index);
            return;
        }
        if self.rating_filter_is_empty() {
            self.set_rating_filter(RatingFilter::All);
            return;
        }
        let empty_outside_filter = self
            .playlist
            .as_ref()
            .is_some_and(|playlist| playlist.outside_filter() && playlist.visible_len() == 0);
        if empty_outside_filter {
            if let Some(playlist) = self.playlist.as_mut() {
                playlist.dismiss_outside_filter();
            }
            self.apply_filter_selection(FilterSelection::Empty);
        }
    }

    fn rating_filter_is_empty(&self) -> bool {
        self.rating_scan_worker.is_none()
            && self
                .playlist
                .as_ref()
                .is_some_and(Playlist::empty_filter_can_show_all)
    }

    fn escape_context(&self) -> EscapeContext {
        EscapeContext {
            context_menu_open: self.context_menu_pos.is_some(),
            is_cropping: self.transform.is_cropping,
            is_healing: self.heal.active,
            is_mosaic: self.mosaic.is_active(),
            empty_rating_filter: self.rating_filter_is_empty(),
            is_fullscreen: self.is_fullscreen,
        }
    }

    fn go_to_index(&mut self, new_index: usize) {
        self.cancel_open_with_check();
        if self.block_browse_while_busy() {
            return;
        }
        self.go_to_index_ready(new_index);
    }

    /// Navigate after a caller has completed its own mutation preflight.
    ///
    /// A submitted source-removal worker intentionally remains active while the
    /// surviving neighbor begins presentation, so this path must not repeat the
    /// ordinary busy check.
    fn go_to_index_ready(&mut self, new_index: usize) {
        let Some(playlist) = &self.playlist else {
            return;
        };
        if playlist.files.is_empty() || new_index >= playlist.files.len() {
            return;
        }
        let next_path = playlist.files[new_index].clone();
        let Some(current_path) = playlist.files.get(playlist.index) else {
            return;
        };
        let plan = navigation_image_plan(
            playlist.index,
            new_index,
            current_path,
            &next_path,
            self.session.presented_path.as_deref(),
            self.current_image.is_some(),
            self.current_image_reuse,
        );
        let retained_image = if plan == NavigationImagePlan::RetainPresented {
            self.current_image.as_ref().and_then(|image| {
                self.current_source
                    .as_ref()
                    .map(|source| (current_path.clone(), Arc::clone(image), Arc::clone(source)))
            })
        } else {
            None
        };

        self.cancel_rating_disclosure_for_source_change();

        let Some(playlist) = self.playlist.as_mut() else {
            return;
        };
        playlist.select(new_index);
        self.session.selected_path = Some(next_path.clone());
        self.transform = Transform::default();

        if plan == NavigationImagePlan::ReusePresented {
            self.cancel_pending_image_load();
            // A retained alias must leave the cache before pixel editing can
            // regain unique ownership of the displayed decode.
            self.remove_prefetched_image(&next_path);
            self.prefetch_schedule.allow(&next_path);
            self.session.set_presented(next_path.clone());
            self.start_auxiliary_load(&next_path);
            self.kick_prefetch();
            self.request_redraw();
            return;
        }

        // Reserve the requested cache entry before retaining the outgoing
        // image, so cache pressure cannot evict the image the user selected.
        let cached_target = self.take_prefetched_image(&next_path);
        if let Some((path, image, source)) = retained_image
            && self.insert_prefetched_image(path.clone(), LoadedImage { image, source })
        {
            // Cancel any speculative decode that could replace the exact
            // trusted pixels retained above.
            self.prefetch_schedule.allow(&path);
        }
        self.spawn_image_load_with_cached(next_path, cached_target, false, false);
        self.kick_prefetch();
    }

    fn advance_after_removal_submitted(&mut self, removed_index: usize) {
        let Some(next_index) = self
            .playlist
            .as_ref()
            .and_then(|playlist| playlist.successor_after_removal(removed_index))
        else {
            return;
        };
        self.go_to_index_ready(next_index);
    }

    /// Decode nearby playlist entries into the in-memory cache (no disk writes).
    fn kick_prefetch(&mut self) {
        let Some(playlist) = &self.playlist else {
            return;
        };
        let candidate_paths: Vec<PathBuf> = if let Some(page) = self.mosaic.page.as_ref() {
            page.indices
                .iter()
                .filter_map(|index| playlist.files.get(*index).cloned())
                .filter(|path| self.session.presented_path.as_deref() != Some(path.as_path()))
                .collect()
        } else {
            playlist.visible_neighbor_paths(2)
        };
        let targets: Vec<(PathBuf, Option<crate::fs::ScanProvenance>)> = candidate_paths
            .into_iter()
            .filter(|p| !self.prefetch.contains(p) && self.prefetch_schedule.is_eligible(p))
            .filter(|p| !self.mosaic.unavailable_paths.contains(p))
            .map(|path| {
                let provenance = playlist.scan_provenance(&path);
                (path, provenance)
            })
            .collect();
        if targets.is_empty() {
            return;
        }

        for (path, provenance) in targets {
            let event_proxy = self.event_proxy.clone();
            let notify = move || {
                let _ = event_proxy.send_event(UserEvent::Wake);
            };
            let _ = if let Some(provenance) = provenance {
                self.prefetch_schedule.request_scanned(
                    path,
                    provenance,
                    notify,
                    crate::decode::schedule_background_decode,
                )
            } else {
                self.prefetch_schedule.request(
                    path,
                    notify,
                    crate::decode::schedule_background_decode,
                )
            };
        }
    }

    fn poll_prefetch(&mut self) {
        let poll = self.prefetch_schedule.poll();
        let completed = poll.made_progress();
        let mut presented = false;
        for completion in poll.into_completions() {
            let (path, result, superseded) = completion.into_parts();
            let selected_with_foreground = self.session.selected_path.as_deref()
                == Some(path.as_path())
                && self.session.is_loading();
            let path_in_playlist = self
                .playlist
                .as_ref()
                .is_some_and(|playlist| playlist.files.iter().any(|item| item == &path));
            let destination = prefetch_destination(
                self.session.selected_path.as_deref(),
                self.session.is_loading() || self.session.load_error.is_some(),
                path_in_playlist,
                &path,
            );
            let mut diagnostic = None;
            let terminal = match result {
                Ok(Some(_)) if superseded => false,
                Ok(Some(image)) => match destination {
                    PrefetchDestination::PresentSelected => {
                        self.session.cancel_pending_load();
                        self.display_loaded_image(&path, image);
                        presented = true;
                        false
                    }
                    PrefetchDestination::CacheNeighbor => {
                        let retained = if self.mosaic.is_active() {
                            self.insert_mosaic_image_if_fits(path.clone(), image)
                        } else {
                            self.insert_prefetched_image(path.clone(), image)
                        };
                        if !retained {
                            if self.mosaic.is_active() {
                                self.mosaic.memory_limited = true;
                                self.mosaic.unavailable_paths.insert(path.clone());
                            }
                            diagnostic = Some(format!(
                                "neighbor prefetch skipped for {} because it exceeds the cache budget",
                                prefetch::privacy_safe_file_name(&path)
                            ));
                        }
                        !retained
                    }
                    PrefetchDestination::Ignore => false,
                },
                Ok(None) => false,
                Err(_) if selected_with_foreground => false,
                Err(failure) => {
                    if self.mosaic.is_active() {
                        self.mosaic.unavailable_paths.insert(path.clone());
                    }
                    diagnostic = Some(format!(
                        "neighbor prefetch failed for {}: {}",
                        prefetch::privacy_safe_file_name(&path),
                        failure.diagnostic_name()
                    ));
                    true
                }
            };
            let effective_terminal = self
                .prefetch_schedule
                .record_outcome(&path, terminal, superseded);
            if effective_terminal && let Some(diagnostic) = diagnostic {
                log::warn!("{diagnostic}");
            }
        }
        if completed {
            self.kick_prefetch();
            if self.show_filmstrip_panel && self.filmstrip_panel_open {
                self.request_thumbs_for_filmstrip();
            }
        }
        if self.mosaic.is_active() {
            self.sync_mosaic_gpu();
        }
        if presented && let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn trash_current(&mut self) {
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        if self.block_action_while_busy("moving this file to Trash") {
            return;
        }
        if let Some(message) = self.curation_recovery.source_removal_preflight() {
            self.show_toast(message);
            return;
        }

        let Some(source) = self.current_source.as_ref().map(Arc::clone) else {
            let error = GuardedActionError::Unavailable;
            log_guarded_action_failure(GuardedSourceAction::Trash, &error);
            self.show_toast(guarded_source_action_failure_message(
                GuardedSourceAction::Trash,
                &error,
            ));
            return;
        };
        let playlist_index = self.playlist.as_ref().map_or(0, |p| p.index);
        let context = CurationContext::Trash(RemovalContext {
            path: path.clone(),
            playlist_index,
            scope: self.playlist_scope.clone(),
        });
        // Persistent top-bar status owns the in-progress state. Outcome toasts
        // fire only when the worker finishes, so Delete does not flash a second
        // busy message on top of the spinner status.
        let started = self.start_curation_worker(
            "viewr-trash-move",
            context,
            move || CurationCompletion::Trash {
                result: crate::curate::move_source_to_trash(&path, &source),
            },
            "Could not start the move to Trash. Nothing was moved.",
        );
        if started {
            self.advance_after_removal_submitted(playlist_index);
        }
    }

    fn start_curation_worker(
        &mut self,
        name: &'static str,
        context: CurationContext,
        work: impl FnOnce() -> CurationCompletion + Send + 'static,
        spawn_failure: &'static str,
    ) -> bool {
        let kind = context.kind();
        let submitted = context.submitted();
        let event_proxy = self.event_proxy.clone();
        let spawn = spawn_curation_thread(name, work, move || {
            let _ = event_proxy.send_event(UserEvent::Wake);
        });
        let Ok((result_rx, join)) = spawn else {
            log::error!("curation worker spawn failed: operation={kind:?}, submitted={submitted}");
            self.show_toast(spawn_failure);
            return false;
        };
        log::info!("curation worker started: operation={kind:?}, submitted={submitted}");
        self.curation_worker = Some(CurationWorker {
            context,
            result_rx,
            join: Some(join),
        });
        self.request_redraw();
        true
    }

    fn active_work(&self, allowance: ActiveModeAllowance) -> [Option<CurrentWork>; 8] {
        let source_verification = self
            .open_with_job
            .is_some()
            .then_some(CurrentWork::SourceVerification);
        let spot_heal = spot_heal_work(
            self.heal.active,
            self.heal.is_busy(),
            self.heal.painting,
            allowance,
        );
        [
            self.curation_worker
                .as_ref()
                .map(|worker| curation_work(worker.context.kind())),
            source_verification,
            self.folder_scan_job
                .is_some()
                .then_some(CurrentWork::FolderScan),
            image_preparation_work(self.session.is_loading(), self.preview_job.is_some()),
            crop_work(
                self.transform.is_cropping,
                self.transform.crop_start.is_some(),
                self.crop_job.is_some(),
                allowance,
            ),
            self.save_transaction_active().then_some(CurrentWork::Save),
            self.rating_write_worker
                .is_some()
                .then_some(CurrentWork::RatingWrite),
            spot_heal,
        ]
    }

    fn busy_blocker(&self, allowance: ActiveModeAllowance) -> Option<CurrentWork> {
        current_work_blocker(self.active_work(allowance))
    }

    fn block_action_with_mode_allowance(
        &mut self,
        action: &str,
        allowance: ActiveModeAllowance,
    ) -> bool {
        if let Some(blocker) = self.busy_blocker(allowance) {
            self.show_toast(blocked_action_message(action, blocker));
            true
        } else {
            false
        }
    }

    fn block_action_while_busy(&mut self, action: &str) -> bool {
        self.block_action_with_mode_allowance(action, ActiveModeAllowance::None)
    }

    fn block_edit_history_while_busy(&mut self, action: &str) -> bool {
        self.block_action_with_mode_allowance(action, ActiveModeAllowance::SpotHeal)
    }

    fn block_browse_while_busy(&mut self) -> bool {
        if let Some(blocker) = browse_work_blocker(self.active_work(ActiveModeAllowance::None)) {
            self.show_toast(blocked_action_message("browsing to another image", blocker));
            true
        } else {
            false
        }
    }

    fn block_action_while_curating(&mut self, action: &str) -> bool {
        let Some(worker) = self.curation_worker.as_ref() else {
            return false;
        };
        let blocker = curation_work(worker.context.kind());
        self.show_toast(blocked_action_message(action, blocker));
        true
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

    fn cancel_crop_work(&mut self) {
        if let Some(job) = self.crop_job.take() {
            job.context().cancel.store(true, Ordering::Release);
            log::debug!("crop work cancelled after source change");
        }
    }

    fn crop_recovery_is_current(&self, recovery: &CropRecovery) -> bool {
        crop_recovery_matches(
            CropRecoveryIdentity {
                path: recovery.source_path.as_path(),
                generation: recovery.source_generation,
                image: &recovery.source_image,
            },
            self.session.generation.load(Ordering::Acquire),
            self.session.selected_path.as_deref(),
            self.session.presented_path.as_deref(),
            self.current_image.as_ref(),
        )
    }

    fn restore_failed_crop(&mut self, recovery: CropRecovery) -> bool {
        if !self.crop_recovery_is_current(&recovery) {
            log::debug!("discarded stale crop recovery state");
            return false;
        }
        let restored = recovery.into_restored_edit_state();
        self.transform = restored.transform;
        self.animation = restored.animation;
        self.pages = restored.pages;
        self.auxiliary_job = restored.auxiliary_job;
        self.request_redraw();
        true
    }

    fn capture_crop_recovery(
        &mut self,
        source_path: PathBuf,
        source_image: Arc<DecodedImage>,
    ) -> CropRecovery {
        CropRecovery {
            source_path,
            source_generation: self.session.generation.load(Ordering::Acquire),
            source_image,
            transform: self.transform,
            animation: self.animation.take(),
            pages: self.pages.take(),
            auxiliary_job: self.auxiliary_job.take(),
        }
    }

    fn toggle_crop_mode(&mut self) {
        if self.transform.is_cropping {
            self.cancel_crop();
            return;
        }
        if let Some(message) = crop_recovery_blocker(
            self.crop_recovery_unsettled,
            self.preview_recovery_unsettled,
        ) {
            self.show_toast(message);
            return;
        }
        if let Some(message) =
            crop_source_blocker(self.session.is_loading(), self.session.load_error.is_some())
        {
            self.show_toast(message);
            return;
        }
        if self.current_loaded_path().is_none() {
            return;
        }
        if self.block_action_with_mode_allowance("changing Crop", ActiveModeAllowance::SpotHeal) {
            return;
        }

        self.heal.active = false;
        self.heal.stroke.clear();
        self.heal.painting = false;
        self.heal.cancel_worker();
        if let Some((show, expanded)) = self.tools_before_heal.take() {
            self.show_tools_panel = show;
            self.tools_panel_open = expanded;
        }
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
        if self.heal.active {
            if self.heal.painting {
                self.finish_heal_stroke();
            }
            self.heal.active = false;
            self.heal.stroke.clear();
            self.heal.painting = false;
            if let Some((show, expanded)) = self.tools_before_heal.take() {
                self.show_tools_panel = show;
                self.tools_panel_open = expanded;
            }
            if self.heal.is_busy() {
                self.show_toast("Finishing spot heal in memory");
            }
            self.request_redraw();
            return;
        }
        if let Some(message) =
            spot_heal_source_blocker(self.session.is_loading(), self.session.load_error.is_some())
        {
            self.show_toast(message);
            return;
        }
        if self.current_loaded_path().is_none() {
            return;
        }
        if self.block_action_with_mode_allowance("changing Spot Heal", ActiveModeAllowance::Crop) {
            return;
        }
        if !self.can_heal_current_image() {
            self.show_toast(
                "Spot Heal is unavailable for images larger than the GPU texture limit",
            );
            return;
        }
        self.pause_animation();
        self.heal.active = true;
        self.heal.stroke.clear();
        self.heal.painting = false;
        self.cancel_crop();
        self.tools_before_heal = Some((self.show_tools_panel, self.tools_panel_open));
        self.show_tools_panel = true;
        self.tools_panel_open = true;
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
        if self.block_edit_history_while_busy("refreshing the heal source") {
            return;
        }
        let Some(refresh) = self.heal.refresh.as_ref() else {
            self.show_toast("Apply a spot heal before refreshing its source");
            return;
        };
        if refresh.candidate_count < 2 {
            self.show_toast("No alternate spot-heal source is available");
            return;
        }
        let candidate_index = (refresh.candidate_index + 1) % refresh.candidate_count;
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
        if !self.heal.active {
            return;
        }
        if self.block_edit_history_while_busy("starting a spot-heal stroke") {
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
            )
        });
        let (output, apply_result, replacing_latest) = match polled {
            Some((Ok(output), apply_result, replacing_latest)) => {
                (Some(output), apply_result, replacing_latest)
            }
            Some((Err(TryRecvError::Disconnected), apply_result, _)) => {
                self.heal.worker = None;
                self.heal.stroke.clear();
                if apply_result {
                    self.show_toast("Spot heal stopped unexpectedly");
                }
                return;
            }
            Some((Err(TryRecvError::Empty), _, _)) | None => (None, false, false),
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
                let apply_result = {
                    let (current_image, renderer, history, refresh) = (
                        &mut self.current_image,
                        &mut self.renderer,
                        &mut self.heal.history,
                        &mut self.heal.refresh,
                    );
                    current_image
                        .as_mut()
                        .and_then(Arc::get_mut)
                        .ok_or(crate::heal::PatchPresentationError::Edit(
                            crate::heal::HealError::InvalidImageBuffer,
                        ))
                        .and_then(|image| {
                            commit_presented_heal(
                                image,
                                history,
                                refresh,
                                &result,
                                output.job,
                                replacing_latest,
                                |image, patch| present_image_patch(renderer.as_mut(), image, patch),
                            )
                        })
                };
                match apply_result {
                    Ok(message) => {
                        self.current_image_reuse = ImageReuseEligibility::Ineligible;
                        self.show_toast(message);
                    }
                    Err(error) => self.report_edit_transaction_failure("Spot heal", &error),
                }
            }
            Err(error) => {
                self.show_toast(format!("Spot heal failed: {error}"));
            }
        }
        self.request_redraw();
    }

    fn undo_edit(&mut self) {
        if !self.heal.history.can_undo() {
            return;
        }
        if self.block_edit_history_while_busy("undoing an edit") {
            return;
        }
        let result = {
            let (current_image, renderer, history) = (
                &mut self.current_image,
                &mut self.renderer,
                &mut self.heal.history,
            );
            current_image
                .as_mut()
                .and_then(Arc::get_mut)
                .ok_or(crate::heal::PatchPresentationError::Edit(
                    crate::heal::HealError::InvalidImageBuffer,
                ))
                .and_then(|image| {
                    history.undo_presented(image, |image, patch| {
                        present_image_patch(renderer.as_mut(), image, patch)
                    })
                })
        };
        match result {
            Ok(true) => {
                self.current_image_reuse = ImageReuseEligibility::Ineligible;
                self.heal.refresh = None;
                self.show_toast("Undid spot heal");
            }
            Err(error) => self.report_edit_transaction_failure("Undo", &error),
            Ok(false) => {}
        }
    }

    fn redo_edit(&mut self) {
        if !self.heal.history.can_redo() {
            return;
        }
        if self.block_edit_history_while_busy("redoing an edit") {
            return;
        }
        let result = {
            let (current_image, renderer, history) = (
                &mut self.current_image,
                &mut self.renderer,
                &mut self.heal.history,
            );
            current_image
                .as_mut()
                .and_then(Arc::get_mut)
                .ok_or(crate::heal::PatchPresentationError::Edit(
                    crate::heal::HealError::InvalidImageBuffer,
                ))
                .and_then(|image| {
                    history.redo_presented(image, |image, patch| {
                        present_image_patch(renderer.as_mut(), image, patch)
                    })
                })
        };
        match result {
            Ok(true) => {
                self.current_image_reuse = ImageReuseEligibility::Ineligible;
                self.heal.refresh = None;
                self.show_toast("Redid spot heal");
            }
            Err(error) => self.report_edit_transaction_failure("Redo", &error),
            Ok(false) => {}
        }
    }

    fn report_edit_transaction_failure(
        &mut self,
        action: &str,
        error: &crate::heal::PatchPresentationError<String>,
    ) {
        log::error!("edit presentation transaction failed during {action}: {error}");
        if error.rollback_failed() {
            if let Some(path) = self.session.selected_path.clone() {
                self.spawn_image_load(path);
                self.show_toast(edit_transaction_failure_message(action, error, true));
            } else {
                self.invalidate_displayed_image();
                self.show_toast(edit_transaction_failure_message(action, error, false));
            }
        } else {
            self.show_toast(edit_transaction_failure_message(action, error, false));
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
        playlist
            .visible_catalog_range()
            .into_iter()
            .filter_map(|i| {
                let position = playlist.visible_position_for_catalog_index(i)?;
                let path = &playlist.files[i];
                let name = prefetch::privacy_safe_file_name(path);
                let texture = self.thumb_textures.get(path).cloned();
                Some(FilmstripItem {
                    index: i,
                    position: position.saturating_add(1),
                    name,
                    texture,
                })
            })
            .collect()
    }

    fn request_thumbs_for_filmstrip(&mut self) {
        let paths = self.visible_filmstrip_paths();
        let visible = paths.iter().cloned().collect::<HashSet<_>>();
        self.thumb_textures.retain(|path, _| visible.contains(path));
        self.thumbnail_schedule.retain_visible_failures(&visible);
        for path in paths {
            if self.thumb_textures.contains_key(&path) {
                continue;
            }
            let provenance = self
                .playlist
                .as_ref()
                .and_then(|playlist| playlist.scan_provenance(&path));
            let event_proxy = self.event_proxy.clone();
            let notify = move || {
                let _ = event_proxy.send_event(UserEvent::Wake);
            };
            if let Some(provenance) = provenance {
                self.thumbnail_schedule.request_scanned(
                    path,
                    provenance,
                    notify,
                    crate::decode::schedule_background_decode,
                );
            } else {
                self.thumbnail_schedule.request(
                    path,
                    notify,
                    crate::decode::schedule_background_decode,
                );
            }
        }
    }

    fn visible_filmstrip_paths(&self) -> Vec<PathBuf> {
        let Some(playlist) = &self.playlist else {
            return Vec::new();
        };
        playlist
            .visible_catalog_range()
            .into_iter()
            .map(|index| playlist.files[index].clone())
            .collect()
    }

    fn poll_thumbnails(&mut self) {
        let visible = if self.show_filmstrip_panel && self.filmstrip_panel_open {
            self.visible_filmstrip_paths()
                .into_iter()
                .collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        self.thumbnail_schedule.retain_visible_failures(&visible);
        let poll = self.thumbnail_schedule.poll(&visible);
        let has_visible_completion = !poll.completions.is_empty();
        for completion in poll.completions {
            match completion {
                ThumbnailCompletion::Ready { path, thumbnail } => {
                    if let Some(renderer) = &self.renderer {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            thumbnail.dimensions(),
                            thumbnail.rgba(),
                        );
                        // Texture names stay path-free so debug dumps and inspector
                        // surfaces cannot retain full filesystem paths.
                        let id = path_free_texture_id("thumb", &path);
                        let handle =
                            renderer
                                .egui_ctx
                                .load_texture(id, image, egui::TextureOptions::LINEAR);
                        self.thumb_textures.insert(path, handle);
                    }
                }
                ThumbnailCompletion::Failed { failure } => {
                    log::debug!(
                        "thumbnail preparation failed: {}",
                        failure.diagnostic_name()
                    );
                }
            }
        }
        if has_visible_completion && let Some(r) = self.renderer.as_ref() {
            r.window().request_redraw();
        }
        if poll.made_progress {
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
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        if self.block_action_while_busy("permanently deleting this file") {
            return;
        }
        if let Some(message) = self.curation_recovery.source_removal_preflight() {
            self.show_toast(message);
            return;
        }
        let Some(source) = self.current_source.as_ref().map(Arc::clone) else {
            let error = GuardedActionError::Unavailable;
            log_guarded_action_failure(GuardedSourceAction::PermanentDelete, &error);
            self.show_toast(guarded_source_action_failure_message(
                GuardedSourceAction::PermanentDelete,
                &error,
            ));
            return;
        };
        if let Err(error) = crate::curate::verify_accepted_source_native(&path, &source) {
            log_guarded_action_failure(GuardedSourceAction::PermanentDelete, &error);
            self.show_toast(guarded_source_action_failure_message(
                GuardedSourceAction::PermanentDelete,
                &error,
            ));
            return;
        }
        let safe_name = prefetch::privacy_safe_file_name(&path).replace('"', "?");
        let confirmed = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Permanently delete?")
            .set_description(permanent_delete_description(&safe_name))
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                PERMANENT_DELETE_ACTION.to_owned(),
                "Cancel".to_owned(),
            ))
            .show();
        let confirmed_label = match &confirmed {
            rfd::MessageDialogResult::Custom(label) => Some(label.as_str()),
            _ => None,
        };
        if !permanent_delete_confirmed(confirmed_label) {
            return;
        }
        let playlist_index = self.playlist.as_ref().map_or(0, |p| p.index);
        let context = CurationContext::PermanentDelete(RemovalContext {
            path: path.clone(),
            playlist_index,
            scope: self.playlist_scope.clone(),
        });
        let started = self.start_curation_worker(
            "viewr-permanent-delete",
            context,
            move || CurationCompletion::PermanentDelete {
                result: crate::curate::permanent_delete_source(&path, &source),
            },
            "Could not start permanent delete. Nothing was deleted.",
        );
        if started {
            self.advance_after_removal_submitted(playlist_index);
            self.show_toast("Permanently deleting file in the background");
        }
    }

    fn finish_trash_move(
        &mut self,
        context: &RemovalContext,
        result: Result<crate::curate::TrashReceipt, GuardedActionError>,
    ) -> CurationTerminalState {
        let receipt = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                log_guarded_action_failure(GuardedSourceAction::Trash, &error);
                self.show_toast(guarded_source_action_failure_message(
                    GuardedSourceAction::Trash,
                    &error,
                ));
                return CurationTerminalState::NeedsAttention;
            }
        };
        let has_receipt = receipt.can_restore_in_app();
        if !has_receipt {
            log::warn!(
                "trash receipt unavailable: category={}",
                receipt.capture_status().category()
            );
        }
        let previous_undo_preserved = !has_receipt && !self.last_trashed.is_empty();
        if has_receipt {
            self.last_trashed = vec![TrashedFile {
                receipt,
                playlist_index: context.playlist_index,
            }];
            self.last_trashed_scope.clone_from(&context.scope);
        } else if previous_undo_preserved {
            rebase_preserved_trash_action(
                &mut self.last_trashed,
                context.scope.as_ref(),
                self.last_trashed_scope.as_ref(),
                &[context.playlist_index],
            );
        }
        if restore_targets_active_playlist(
            self.playlist.as_ref(),
            self.playlist_scope.as_ref(),
            context.scope.as_ref(),
        ) {
            self.after_paths_removed(std::slice::from_ref(&context.path), context.playlist_index);
        }
        self.show_toast(single_trash_result_message(
            has_receipt,
            previous_undo_preserved,
        ));
        CurationTerminalState::Succeeded
    }

    fn finish_permanent_delete(
        &mut self,
        context: &RemovalContext,
        result: Result<(), GuardedActionError>,
    ) -> CurationTerminalState {
        if let Err(error) = result {
            log_guarded_action_failure(GuardedSourceAction::PermanentDelete, &error);
            self.show_toast(guarded_source_action_failure_message(
                GuardedSourceAction::PermanentDelete,
                &error,
            ));
            return CurationTerminalState::NeedsAttention;
        }
        let previous_trash_undo = !self.last_trashed.is_empty();
        if previous_trash_undo {
            rebase_preserved_trash_action(
                &mut self.last_trashed,
                context.scope.as_ref(),
                self.last_trashed_scope.as_ref(),
                &[context.playlist_index],
            );
        }
        if restore_targets_active_playlist(
            self.playlist.as_ref(),
            self.playlist_scope.as_ref(),
            context.scope.as_ref(),
        ) {
            self.after_paths_removed(std::slice::from_ref(&context.path), context.playlist_index);
        }
        let safe_name = prefetch::privacy_safe_file_name(&context.path).replace('"', "?");
        self.show_toast(permanent_delete_success_message(
            &safe_name,
            previous_trash_undo,
        ));
        CurationTerminalState::Succeeded
    }

    fn after_paths_removed(&mut self, removed: &[PathBuf], old_index: usize) {
        // Keep neighbor prefetch for surviving images. Full cache reset made Delete
        // feel like a cold navigation even when the next image was already decoded.
        for path in removed {
            self.remove_prefetched_image(path);
            self.thumb_textures.remove(path);
            self.prefetch_schedule.allow(path);
        }
        let Some(playlist) = self.playlist.as_mut() else {
            self.cancel_pending_image_load();
            self.session.selected_path = None;
            self.invalidate_displayed_image();
            return;
        };

        // Prefer the image the user is already looking at. Trash may finish after
        // they navigated away; never yank them back to the deleted item's slot.
        let selected_before = self
            .session
            .selected_path
            .clone()
            .or_else(|| self.session.presented_path.clone());
        let selected_was_removed = selected_before
            .as_ref()
            .is_some_and(|path| removed.iter().any(|removed_path| removed_path == path));

        playlist.remove_paths(removed, old_index);
        if playlist.files.is_empty() {
            self.cancel_pending_image_load();
            self.session.selected_path = None;
            self.invalidate_displayed_image();
            return;
        }
        if playlist.visible_len() == 0 {
            self.cancel_pending_image_load();
            self.session.selected_path = None;
            self.invalidate_displayed_image();
            return;
        }

        if !selected_was_removed
            && let Some(path) = selected_before
            && let Some(index) = playlist.files.iter().position(|entry| entry == &path)
            && playlist.select(index)
        {
            // Keep the image the user already moved to; only refresh playlist
            // bookkeeping and neighbor work.
            self.session.selected_path = Some(path);
            self.kick_prefetch();
            self.request_redraw();
            return;
        }

        let next_path = playlist.files[playlist.index].clone();
        self.cancel_rating_disclosure_for_source_change();
        self.session.selected_path = Some(next_path.clone());
        self.transform = Transform::default();
        self.spawn_image_load(next_path);
        self.kick_prefetch();
    }

    fn handle_missing_selected_path(&mut self, path: PathBuf) {
        self.session.set_selected_missing();
        self.show_toast(crate::session::MISSING_IMAGE_STATUS);

        let old_index = self
            .playlist
            .as_ref()
            .and_then(|playlist| playlist.files.iter().position(|entry| entry == &path));
        let Some(old_index) = old_index else {
            self.start_missing_selection_scan(path);
            self.request_redraw();
            return;
        };

        self.remove_prefetched_image(&path);
        self.thumb_textures.remove(&path);
        self.prefetch_schedule.allow(&path);
        let removal = {
            let playlist = self
                .playlist
                .as_mut()
                .expect("located missing selection belongs to the active playlist");
            playlist.remove_paths(std::slice::from_ref(&path), old_index);
            if playlist.files.is_empty() {
                MissingSelectionRemoval::ScanFolder
            } else if playlist.visible_len() == 0 {
                MissingSelectionRemoval::FilterEmpty
            } else {
                MissingSelectionRemoval::Advance(playlist.files[playlist.index].clone())
            }
        };

        match removal {
            MissingSelectionRemoval::Advance(next_path) => {
                self.cancel_rating_disclosure_for_source_change();
                self.session.selected_path = Some(next_path.clone());
                self.transform = Transform::default();
                self.spawn_image_load(next_path);
                self.kick_prefetch();
            }
            MissingSelectionRemoval::ScanFolder => {
                // The stale selection may have been the only installed entry.
                // Keep the prior frame while a fresh parent scan looks for a
                // surviving sibling.
                self.start_missing_selection_scan(path);
            }
            MissingSelectionRemoval::FilterEmpty => {
                self.cancel_pending_image_load();
                self.session.selected_path = None;
                self.invalidate_displayed_image();
                self.show_toast(
                    "The selected image is no longer available, and no remaining image matches the rating filter.",
                );
            }
        }
        self.request_redraw();
    }

    fn start_missing_selection_scan(&mut self, path: PathBuf) {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        self.start_folder_scan(
            directory,
            ScanPurpose::SelectedFile {
                path,
                missing_recovery: true,
            },
        );
    }

    fn undo_trash(&mut self) {
        let active = self
            .curation_worker
            .as_ref()
            .map(|worker| worker.context.kind());
        if let Some(message) = curation_action_preflight(
            active,
            !self.last_trashed.is_empty(),
            "restoring files from Trash",
            "Nothing to restore from Trash",
        ) {
            self.show_toast(message);
            return;
        }
        if self.block_action_while_busy("restoring files from Trash") {
            return;
        }

        let records = self.last_trashed.clone();
        let restore_submitted = records.len();
        let context = CurationContext::Restore(RestoreContext {
            submitted: restore_submitted,
            scope: self.last_trashed_scope.clone(),
        });
        let event_proxy = self.event_proxy.clone();
        let spawn = spawn_curation_thread(
            "viewr-trash-restore",
            move || {
                let restore_started = Instant::now();
                let mut outcome = crate::curate::restore_trash_batch(records);
                let evidence = inspect_restored_entries(&mut outcome);
                CurationCompletion::Restore {
                    outcome,
                    evidence,
                    elapsed: restore_started.elapsed(),
                }
            },
            move || {
                let _ = event_proxy.send_event(UserEvent::Wake);
            },
        );
        let Ok((result_rx, join)) = spawn else {
            log::error!("trash restore worker spawn failed: submitted={restore_submitted}");
            self.show_toast(
                "Could not start Trash restore. Undo receipts are unchanged; retry with U.",
            );
            return;
        };
        log::info!("trash restore worker started: submitted={restore_submitted}");
        self.curation_worker = Some(CurationWorker {
            context,
            result_rx,
            join: Some(join),
        });
        self.request_redraw();
    }

    fn finish_trash_restore(
        &mut self,
        context: RestoreContext,
        mut outcome: crate::curate::TrashRestoreOutcome,
        evidence: Vec<RestoredEntryEvidence>,
        restore_elapsed: Duration,
    ) -> CurationTerminalState {
        let restores_active_playlist = restore_targets_active_playlist(
            self.playlist.as_ref(),
            self.playlist_scope.as_ref(),
            context.scope.as_ref(),
        );
        let restored_count = outcome.restored.len();
        if restored_count > 0 && restores_active_playlist {
            self.reset_prefetch_for_playlist_change();
        }
        let mut restored_selection = None;
        if restores_active_playlist && let Some(playlist) = &mut self.playlist {
            let previously_selected = playlist.index;
            let mut focused_index = None;
            let mut evidence = evidence.into_iter();
            for record in &outcome.restored {
                let index = outcome
                    .restored_playlist_index(record.playlist_index)
                    .min(playlist.files.len());
                focused_index.get_or_insert(index);
                let path = record.receipt.original_path().to_owned();
                let evidence = evidence.next();
                let (rating, provenance) = if let Some(evidence) = evidence
                    && evidence.path == path
                {
                    (evidence.rating, evidence.provenance)
                } else {
                    (RatingState::Unreadable, None)
                };
                playlist.insert_path(index, path, rating, provenance);
            }
            if let Some(index) = focused_index {
                playlist.index = index.min(playlist.files.len().saturating_sub(1));
                let filter = playlist.filter();
                restored_selection = Some(match playlist.set_filter(filter) {
                    FilterSelection::Stay => FilterSelection::Select(playlist.index),
                    selection => selection,
                });
                playlist.index = previously_selected.min(playlist.files.len().saturating_sub(1));
            }
        }
        let retry_now = outcome.failure_count(TrashRestoreDisposition::RetryNow);
        let resolve_then_retry = outcome.failure_count(TrashRestoreDisposition::ResolveThenRetry);
        let manual_review = outcome.failure_count(TrashRestoreDisposition::ManualReview);
        let terminal = outcome.failure_count(TrashRestoreDisposition::Terminal);
        let first_failure = outcome.first_failure();
        let failure_total = retry_now + resolve_then_retry + manual_review + terminal;
        log::info!(
            "trash restore timing: submitted={}, restored={restored_count}, failures={failure_total}, total_ms={}",
            context.submitted,
            restore_elapsed.as_millis()
        );
        if failure_total > 0 {
            let first_failure_category = first_failure.map_or("none", |error| error.category());
            log::warn!(
                "trash restore guarded result: restored={restored_count}, retry_now={retry_now}, resolve_then_retry={resolve_then_retry}, manual_review={manual_review}, terminal={terminal}, first_failure_category={first_failure_category}"
            );
        }
        self.last_trashed = outcome.take_retryable_records();
        if self.last_trashed.is_empty() {
            self.last_trashed_scope = None;
        } else {
            self.last_trashed_scope = context.scope;
        }

        if let Some(selection) = restored_selection {
            self.apply_filter_selection(selection);
        }

        self.show_toast(restore_result_message(
            restored_count,
            retry_now,
            resolve_then_retry,
            manual_review,
            terminal,
            first_failure,
            restores_active_playlist,
        ));
        if failure_total == 0 {
            CurationTerminalState::Succeeded
        } else {
            CurationTerminalState::NeedsAttention
        }
    }

    fn spawn_image_load(&mut self, path: PathBuf) {
        self.spawn_image_load_with_recovery(path, false);
    }

    fn spawn_image_load_with_recovery(&mut self, path: PathBuf, preserve_missing_recovery: bool) {
        let cached_image = self.take_prefetched_image(&path);
        self.spawn_image_load_with_cached(path, cached_image, false, preserve_missing_recovery);
    }

    fn spawn_refreshed_image_load(&mut self, path: PathBuf) {
        self.cancel_rating_disclosure_for_source_change();
        self.spawn_image_load_with_cached(path, None, true, false);
    }

    fn spawn_image_load_with_cached(
        &mut self,
        path: PathBuf,
        cached_image: Option<LoadedImage>,
        refresh_scanned: bool,
        preserve_missing_recovery: bool,
    ) {
        let generation = self
            .session
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.prepare_for_image_load(preserve_missing_recovery);

        // Prefer RAM cache even for non-navigate loads (undo, filmstrip jump).
        if let Some(image) = cached_image {
            self.display_loaded_image(&path, image);
            self.session.receiver = None;
            self.kick_prefetch();
            if let Some(r) = self.renderer.as_ref() {
                r.window().request_redraw();
            }
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.session.receiver = Some(rx);
        let event_proxy = self.event_proxy.clone();
        let current_generation = Arc::clone(&self.session.generation);
        let provenance = self
            .playlist
            .as_ref()
            .and_then(|playlist| playlist.scan_provenance(&path));
        let scheduled = crate::decode::schedule_foreground_decode(move || {
            let loaded = match (refresh_scanned, provenance) {
                (true, Some(_)) => DecodedImage::load_refreshed_regular_if_current(
                    &path,
                    &current_generation,
                    generation,
                ),
                (_, Some(provenance)) => DecodedImage::load_scanned_if_current(
                    &path,
                    provenance,
                    &current_generation,
                    generation,
                ),
                (_, None) => DecodedImage::load_if_current(&path, &current_generation, generation),
            };
            let res = match loaded {
                Ok(Some(image)) => Ok(image),
                Ok(None) => return,
                Err(error) => {
                    let error = error.to_string();
                    if crate::fs::path_is_definitely_missing(&path) {
                        Err(ForegroundLoadFailure::MissingCandidate(error))
                    } else {
                        Err(ForegroundLoadFailure::Other(error))
                    }
                }
            };
            let _ = tx.send((path, res));
            let _ = event_proxy.send_event(UserEvent::Wake);
        });
        if let Err(error) = scheduled {
            self.session.receiver = None;
            log::error!("failed to queue foreground decode");
            let message = format!("Could not start image decode: {error}");
            self.session.load_error = Some(message.clone());
            self.show_toast(message);
        }
    }

    fn poll_foreground_image_load(&mut self) {
        let Some(polled) = self.session.receiver.as_ref().map(poll_worker) else {
            return;
        };
        let (path, result) = match polled {
            WorkerPoll::Pending => return,
            WorkerPoll::Ready(completion) => completion,
            WorkerPoll::Disconnected => {
                self.session.receiver = None;
                let message = crate::session::FOREGROUND_EXECUTOR_LOSS_STATUS.to_owned();
                self.session.load_error = Some(message.clone());
                log::error!("foreground image result channel disconnected");
                if self.current_image.is_some() {
                    self.show_toast(decode_failure_toast(&message, true));
                }
                self.request_redraw();
                return;
            }
        };

        self.session.receiver = None;
        if self.session.selected_path.as_ref() != Some(&path) {
            return;
        }
        match result {
            Ok(image) => {
                self.display_loaded_image(&path, image);
                self.kick_prefetch();
                self.request_redraw();
            }
            Err(failure) => {
                let disposition = resolve_foreground_load_failure(
                    failure,
                    crate::fs::path_is_definitely_missing(&path),
                    self.session.presented_path.as_ref() == Some(&path),
                );
                match disposition {
                    ForegroundLoadFailureDisposition::MissingSelection => {
                        log::info!("selected image disappeared before presentation");
                        self.handle_missing_selected_path(path);
                    }
                    ForegroundLoadFailureDisposition::Other(error) => {
                        self.session.selected_missing = false;
                        log::error!("decode failed");
                        let message = user_facing_decode_error(error);
                        self.session.load_error = Some(message.clone());
                        if self.current_image.is_some() {
                            self.show_toast(decode_failure_toast(&message, true));
                        }
                        self.request_redraw();
                    }
                }
            }
        }
    }

    fn save_transaction_active(&self) -> bool {
        self.pending_save.is_some() || self.save_job.is_some()
    }

    fn cancel_save_overwrite_for_source_change(&mut self) {
        if cancel_pending_save_for_source_change(&mut self.pending_save) {
            self.show_toast(
                "Pending Save As overwrite canceled because the active image selection changed.",
            );
        }
    }

    fn cancel_rating_disclosure_for_source_change(&mut self) {
        if cancel_pending_rating_for_source_change(&mut self.pending_rating_write) {
            self.show_toast(
                "Pending rating change canceled because the active image was reopened or changed.",
            );
        }
    }

    fn save_as(&mut self) {
        if self.block_action_while_curating("saving a copy") {
            return;
        }
        if let Some(blocker) = save_start_blocker([
            self.save_recovery_unsettled
                .then_some(SaveStartBlocker::Recovery),
            folder_scan_blocks_save(
                self.folder_scan_job
                    .as_ref()
                    .and_then(|job| job.context().purpose.as_ref()),
            )
            .then_some(SaveStartBlocker::FolderOpen),
            self.rating_write_worker
                .is_some()
                .then_some(SaveStartBlocker::RatingWrite),
            self.preview_job
                .is_some()
                .then_some(SaveStartBlocker::Preview),
            (self.heal.is_busy() || self.heal.painting).then_some(SaveStartBlocker::SpotHeal),
            self.crop_job.is_some().then_some(SaveStartBlocker::Crop),
            self.transform
                .is_cropping
                .then_some(SaveStartBlocker::CropSelection),
            self.save_transaction_active()
                .then_some(SaveStartBlocker::Save),
        ]) {
            self.show_toast(save_start_blocker_message(blocker));
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
        let destination = match crate::edit::prepare_save_destination(&save_path) {
            Ok(destination) => destination,
            Err(error) => {
                self.show_toast(format!("Save failed: {error}"));
                return;
            }
        };
        let options = if self.retain_exif {
            crate::edit::SaveOptions::retain_exif()
        } else {
            crate::edit::SaveOptions::strip()
        };
        let pending = PendingSave {
            source_path: path,
            source_image: image,
            source: self.current_source.as_ref().map(Arc::clone),
            destination,
            pixel_transform,
            options,
        };
        if pending.destination.requires_overwrite_confirmation() {
            self.pending_save = Some(pending);
            self.request_redraw();
        } else {
            self.start_save(pending);
        }
    }

    fn confirm_save_overwrite(&mut self) {
        let Some(pending) = self.pending_save.take() else {
            return;
        };
        if let Err(error) = pending.destination.confirm_overwrite() {
            self.show_toast(format!("Save failed: {error}"));
            return;
        }
        self.start_save(pending);
    }

    fn cancel_save_overwrite(&mut self) {
        if self.pending_save.take().is_some() {
            self.show_toast("Save canceled. No file was changed.");
        }
    }

    fn start_save(&mut self, pending: PendingSave) {
        let PendingSave {
            source_path,
            source_image,
            source,
            destination,
            pixel_transform,
            options,
        } = pending;
        let event_proxy = self.event_proxy.clone();
        let (completion, job) = OneShotJob::new((), move || {
            let _ = event_proxy.send_event(UserEvent::Wake);
        });
        let spawn = std::thread::Builder::new()
            .name("viewr-save".into())
            .spawn(move || {
                let result = (|| {
                    let transformed = (!pixel_transform.is_identity())
                        .then(|| pixel_transform.apply(source_image.as_ref()))
                        .transpose()
                        .map_err(|error| error.to_string())?;
                    let export_image = transformed.as_ref().unwrap_or(source_image.as_ref());
                    crate::edit::save_with_accepted_source(
                        export_image,
                        &destination,
                        &source_path,
                        source.as_deref(),
                        options,
                    )
                    .map_err(|error| error.to_string())
                })();
                let _ = completion.complete(result);
            });
        match spawn {
            Ok(_) => {
                self.save_job = Some(job);
                self.show_toast("Saving copy in the background");
            }
            Err(error) => {
                log::error!("failed to start save worker");
                self.show_toast(format!("Could not start save: {error}"));
            }
        }
    }

    fn poll_save_result(&mut self, event_loop: &ActiveEventLoop) {
        let Some(job) = self.save_job.as_ref() else {
            return;
        };
        let polled = job.poll();
        if matches!(polled, JobPoll::Pending) {
            return;
        }
        self.save_job
            .take()
            .expect("save job exists after polling it")
            .into_context();
        let close_requested = std::mem::take(&mut self.close_after_save);
        let includes_pixel_edits = self.unsaved_crop || self.heal.history.can_undo();
        let terminal = match polled {
            JobPoll::Ready(Ok(metadata)) => {
                self.show_toast(save_success_message(metadata, includes_pixel_edits));
                SaveTerminalState::Succeeded
            }
            JobPoll::Ready(Err(error)) => {
                log::error!("failed to save image");
                self.show_toast(format!("Save failed: {error}"));
                SaveTerminalState::Failed
            }
            JobPoll::Disconnected => {
                log::error!("save job disconnected before publishing a result");
                self.save_recovery_unsettled = true;
                self.show_toast(crate::ui::SAVE_RECOVERY_STATUS);
                SaveTerminalState::Disconnected
            }
            JobPoll::Pending => unreachable!("pending save result returned early"),
        };
        if matches!(terminal, SaveTerminalState::Succeeded) {
            self.refresh_folder_membership();
        }
        match save_close_disposition(close_requested, terminal, self.curation_worker.is_some()) {
            SaveCloseDisposition::StayOpen => {}
            SaveCloseDisposition::Exit => event_loop.exit(),
            SaveCloseDisposition::WaitForCuration => {
                self.close_after_curation = true;
            }
            SaveCloseDisposition::CancelDeferredClose => self.close_after_curation = false,
        }
    }

    fn poll_curation_result(&mut self, event_loop: &ActiveEventLoop) {
        let poll = self
            .curation_worker
            .as_ref()
            .map(|worker| poll_worker(&worker.result_rx));
        let Some(poll) = poll else {
            return;
        };
        if matches!(poll, WorkerPoll::Pending) {
            return;
        }

        let mut worker = self
            .curation_worker
            .take()
            .expect("a completed curation poll retains its worker");
        let kind = worker.context.kind();
        let submitted = worker.context.submitted();
        if let Some(join) = worker.join.take()
            && join.join().is_err()
        {
            log::error!(
                "curation worker panicked after terminal channel state: operation={kind:?}, submitted={submitted}"
            );
        }

        match poll {
            WorkerPoll::Ready(completion) => {
                let terminal = match (worker.context, completion) {
                    (CurationContext::Trash(context), CurationCompletion::Trash { result }) => {
                        self.curation_recovery.clear(CurationKind::Trash);
                        self.finish_trash_move(&context, result)
                    }
                    (
                        CurationContext::PermanentDelete(context),
                        CurationCompletion::PermanentDelete { result },
                    ) => {
                        self.curation_recovery.clear(CurationKind::PermanentDelete);
                        self.finish_permanent_delete(&context, result)
                    }
                    (
                        CurationContext::Restore(context),
                        CurationCompletion::Restore {
                            outcome,
                            evidence,
                            elapsed,
                        },
                    ) => {
                        self.curation_recovery.clear(CurationKind::Restore);
                        self.finish_trash_restore(context, outcome, evidence, elapsed)
                    }
                    _ => {
                        self.close_after_curation = false;
                        self.close_after_save = false;
                        log::error!(
                            "curation worker returned a mismatched completion: operation={kind:?}, submitted={submitted}"
                        );
                        let message = curation_recovery_message(kind);
                        self.curation_recovery.record(kind);
                        self.show_toast(message);
                        return;
                    }
                };
                log::info!("curation worker reconciled: operation={kind:?}, submitted={submitted}");
                self.request_redraw();
                match curation_close_disposition(
                    std::mem::take(&mut self.close_after_curation),
                    terminal,
                    self.save_job.is_some(),
                ) {
                    CurationCloseDisposition::StayOpen => {}
                    CurationCloseDisposition::Exit => event_loop.exit(),
                    CurationCloseDisposition::WaitForSave => self.close_after_save = true,
                    CurationCloseDisposition::CancelDeferredClose => {
                        self.close_after_save = false;
                    }
                }
            }
            WorkerPoll::Disconnected => {
                self.close_after_curation = false;
                self.close_after_save = false;
                log::error!(
                    "curation worker disconnected before a result: operation={kind:?}, submitted={submitted}"
                );
                let message = curation_recovery_message(kind);
                self.curation_recovery.record(kind);
                self.show_toast(message);
            }
            WorkerPoll::Pending => unreachable!("pending workers return before being taken"),
        }
    }

    /// Convert a UV crop rect into one bounded pixel rectangle. Locked ratios
    /// are quantized as whole multiples of their reduced integer components,
    /// so the exported pixel dimensions keep the ratio exactly.
    fn apply_crop_rect(&mut self) {
        if let Some(message) = crop_recovery_blocker(
            self.crop_recovery_unsettled,
            self.preview_recovery_unsettled,
        ) {
            self.show_toast(message);
            return;
        }
        if let Some(message) =
            crop_source_blocker(self.session.is_loading(), self.session.load_error.is_some())
        {
            self.show_toast(message);
            return;
        }
        let Some(source_path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        if self.block_action_with_mode_allowance("applying the crop", ActiveModeAllowance::Crop) {
            return;
        }
        let Some(rect) = self.transform.crop_rect else {
            return;
        };
        let Some(image) = self.current_image.as_ref().map(Arc::clone) else {
            return;
        };
        let ratio = crop_ratio_for_source(self.transform.crop_ratio, self.transform.rotation_steps);
        let Some(pixel_rect) = crate::crop::crop_pixel_rect(rect, image.width, image.height, ratio)
        else {
            self.show_toast("The selected ratio is too large for this image");
            return;
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let recovery = self.capture_crop_recovery(source_path, Arc::clone(&image));
        let notify_proxy = self.event_proxy.clone();
        let (completion, job) = OneShotJob::new(CropJobContext { recovery, cancel }, move || {
            let _ = notify_proxy.send_event(UserEvent::Wake);
        });
        let spawn = std::thread::Builder::new()
            .name("viewr-crop".into())
            .spawn(move || {
                let result =
                    match crate::edit::crop_cancellable(image.as_ref(), pixel_rect, &worker_cancel)
                    {
                        Ok(Some(cropped)) => CropJobResult::Completed(cropped),
                        Ok(None) => {
                            log::debug!("crop worker stopped after cancellation");
                            CropJobResult::Cancelled
                        }
                        Err(error) => CropJobResult::Failed(error.to_string()),
                    };
                if !completion.complete(result) {
                    log::debug!("discarded crop result after owner cancellation");
                }
            });
        if let Err(error) = spawn {
            log::error!("crop worker spawn failed: {error}");
            let context = job.into_context();
            self.animation = context.recovery.animation;
            self.pages = context.recovery.pages;
            self.auxiliary_job = context.recovery.auxiliary_job;
            self.show_toast("Could not start crop. Selection kept; press Enter to try again.");
            return;
        }

        self.transform.zoom = 1.0;
        self.transform.offset_x = 0.0;
        self.transform.offset_y = 0.0;
        self.transform.is_panning = false;
        self.transform.last_cursor = None;
        self.transform.crop_rect = None;
        self.transform.is_cropping = false;
        self.transform.crop_start = None;
        self.crop_job = Some(job);
        self.show_toast("Applying crop in the background");
        self.request_redraw();
    }

    fn poll_crop_result(&mut self) {
        let Some(job) = self.crop_job.as_ref() else {
            return;
        };
        let polled = job.poll();
        if matches!(&polled, JobPoll::Pending) {
            return;
        }
        let context = self
            .crop_job
            .take()
            .expect("crop job exists after polling it")
            .into_context();
        let recovery = context.recovery;
        let cropped = match polled {
            JobPoll::Ready(CropJobResult::Completed(cropped)) => cropped,
            JobPoll::Ready(CropJobResult::Failed(error)) => {
                log::error!("crop computation failed: {error}");
                let restored = self.restore_failed_crop(recovery);
                self.show_toast(crop_failure_message(restored));
                return;
            }
            JobPoll::Ready(CropJobResult::Cancelled) => {
                log::debug!("crop computation reached the event loop after cancellation");
                self.restore_failed_crop(recovery);
                return;
            }
            JobPoll::Disconnected => {
                log::error!("crop job disconnected before publishing a result");
                let restored = self.restore_failed_crop(recovery);
                self.crop_recovery_unsettled = true;
                self.show_toast(crop_disconnect_message(restored));
                return;
            }
            JobPoll::Pending => unreachable!("pending crop result returned early"),
        };

        if !self.crop_recovery_is_current(&recovery) {
            log::debug!("discarded stale crop computation result");
            return;
        }
        let source_path = recovery.source_path.clone();
        self.present_cropped_image(&source_path, Arc::new(cropped), recovery);
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
            || self.session.receiver.is_some()
            || self.preview_job.is_some()
            || self.auxiliary_job.is_some()
            || self.crop_job.is_some()
            || self.folder_scan_job.is_some()
        {
            return false;
        }
        true
    }

    fn performance_probe_is_settled(&self) -> bool {
        if !self.performance_probe_has_presented_current()
            || !self.prefetch_schedule.is_idle()
            || !self.thumbnail_schedule.is_idle()
            || self.auxiliary_job.is_some()
            || !performance_ui_is_settled(self.egui_repaint_at)
        {
            return false;
        }
        // The filmstrip is the final asynchronous presentation surface. The
        // idle observation begins only after its visible textures are ready and
        // egui has no delayed hover or activation repaint outstanding.
        self.visible_filmstrip_paths().iter().all(|path| {
            self.thumb_textures.contains_key(path)
                || self.thumbnail_schedule.has_terminal_failure(path)
        })
    }

    fn fail_performance_probe(&mut self, event_loop: &ActiveEventLoop, message: String) {
        if let Some(probe) = self.performance_probe.as_mut() {
            probe.outcome = Some(Err(message));
        }
        event_loop.exit();
    }

    fn performance_probe_adapter(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Option<crate::performance::GpuAdapterReport> {
        if let Some(renderer) = self.renderer.as_ref() {
            return Some(renderer.performance_adapter().clone());
        }
        self.fail_performance_probe(event_loop, "probe never initialized a GPU adapter".into());
        None
    }

    fn complete_performance_probe(&mut self, event_loop: &ActiveEventLoop, playlist_len: usize) {
        let Some(adapter) = self.performance_probe_adapter(event_loop) else {
            return;
        };
        let probe = self.performance_probe.as_ref().unwrap();
        let Some(window_ready) = probe.window_ready else {
            self.fail_performance_probe(event_loop, "probe never observed a visible window".into());
            return;
        };
        let Some(first_pixel) = probe.first_pixel else {
            self.fail_performance_probe(event_loop, "probe never presented an image frame".into());
            return;
        };
        let (idle_window_focused, idle_pointer_inside) = idle_window_state(self.renderer.as_ref());
        let report = crate::performance::PerformanceReport {
            adapter_backend: adapter.backend,
            adapter_name: adapter.name,
            adapter_device_type: adapter.device_type,
            adapter_driver: adapter.driver,
            window_ready_us: crate::performance::duration_us(window_ready),
            first_pixel_us: crate::performance::duration_us(first_pixel),
            max_navigation_us: crate::performance::duration_us(probe.max_navigation),
            idle_redraws: probe.idle_redraws,
            idle_non_redraw_events: probe.idle_non_redraw_events,
            idle_event_repaint_requests: probe.idle_event_repaint_requests,
            idle_scheduled_egui_repaints: probe.idle_scheduled_egui_repaints,
            idle_window_focused,
            idle_pointer_inside,
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
            self.folder_scan_job.is_some(),
            self.session.receiver.is_some() || self.preview_job.is_some(),
            self.auxiliary_job.is_some(),
            self.performance_probe
                .as_ref()
                .is_some_and(|probe| probe.navigation_target.is_some()),
            remaining_navigation,
            presented_current,
            self.performance_probe
                .as_ref()
                .is_some_and(|probe| probe.idle_until.is_some()),
            self.prefetch_schedule.in_flight_len(),
            self.thumbnail_schedule.in_flight_len(),
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

        self.complete_performance_probe(event_loop, playlist_len);
    }
}

/// The monitor the first window will most likely open on.
fn first_monitor(event_loop: &ActiveEventLoop) -> Option<winit::monitor::MonitorHandle> {
    event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())
}

/// Logical size of `monitor`, or `None` when it reports unusable geometry.
///
/// winit reports monitor extents but not the work area a taskbar, dock, or panel
/// leaves behind, so the size policy in `startup` treats this as an upper bound
/// rather than as space viewr may fill.
fn monitor_logical_size(monitor: &winit::monitor::MonitorHandle) -> Option<(f64, f64)> {
    let scale = monitor.scale_factor();
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let size = monitor.size();
    Some((
        f64::from(size.width) / scale,
        f64::from(size.height) / scale,
    ))
}

/// Place the first window inside its monitor instead of where the platform
/// would cascade it.
///
/// A cascaded origin plus a tall window puts the lower edge behind a dock or
/// taskbar even after the size is bounded. Wayland ignores client positioning,
/// which is why the size bound rather than this placement is the contract.
fn placed_window_attributes(
    attributes: WindowAttributes,
    monitor: &winit::monitor::MonitorHandle,
    monitor_size: (f64, f64),
    window_size: (f64, f64),
) -> WindowAttributes {
    let (left, top) = crate::startup::window_position(monitor_size, window_size);
    let scale = monitor.scale_factor();
    let origin = monitor.position();
    attributes.with_position(PhysicalPosition::new(
        origin.x + (left * scale) as i32,
        origin.y + (top * scale) as i32,
    ))
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
        let monitor = first_monitor(event_loop);
        let monitor_size = monitor.as_ref().and_then(monitor_logical_size);
        let window_size = crate::startup::default_window_size(monitor_size);
        let mut attrs = Window::default_attributes()
            .with_title("viewr")
            .with_inner_size(LogicalSize::new(window_size.0, window_size.1))
            .with_min_inner_size(LogicalSize::new(
                crate::startup::MINIMUM_WINDOW_SIZE.0,
                crate::startup::MINIMUM_WINDOW_SIZE.1,
            ))
            .with_theme(self.theme_preference.window_theme())
            .with_visible(false);

        if let (Some(monitor), Some(monitor_size)) = (monitor.as_ref(), monitor_size) {
            attrs = placed_window_attributes(attrs, monitor, monitor_size, window_size);
        }

        if let Some(icon) = load_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("failed to create window: {e}");
                self.startup_failure = Some(Error::Launch(format!(
                    "cannot open a window on this display: {e}"
                )));
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
        match pollster::block_on(Renderer::new(
            window,
            event_loop.owned_display_handle(),
            mode,
            max_base_pixels,
        )) {
            Ok(renderer) => {
                #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
                let mut renderer = renderer;
                #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
                renderer.init_accessibility(event_loop, self.event_proxy.clone());
                self.renderer = Some(renderer);
                self.observe_current_display();
                if let Some(image) = self.current_image.as_ref() {
                    let image = Arc::clone(image);
                    let source = self.current_source.clone();
                    let path = self.session.presented_path.clone();
                    if let Some(path) = path
                        && self
                            .renderer
                            .as_ref()
                            .is_some_and(|renderer| renderer.required_preview(&image).is_some())
                    {
                        self.present_image(&path, image, source, PresentationKind::Loaded);
                    } else if let Some(renderer) = self.renderer.as_mut()
                        && let Err(error) = renderer.set_image(&image, None)
                    {
                        log::error!("failed to upload initial image: {error}");
                    }
                }
                if let Some(notice) = self.preference_recovery_notice.take() {
                    self.show_toast(notice);
                }
                let _ = self
                    .renderer
                    .as_mut()
                    .unwrap()
                    .render(None, None, &[], |_| {});
                {
                    let window = self.renderer.as_ref().unwrap().window();
                    window.set_visible(true);
                    window.request_redraw();
                }
            }
            Err(e) => {
                log::error!("failed to initialize gpu: {e}");
                self.startup_failure = Some(Error::Launch(
                    crate::startup::host_gpu_failure_message(&e.to_string()),
                ));
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

        let is_redraw_event = matches!(&event, WindowEvent::RedrawRequested);
        let is_own_window = self
            .renderer
            .as_ref()
            .is_some_and(|renderer| renderer.window().id() == window_id);

        let mut egui_consumed = false;
        let mut egui_popup_open = false;
        let mut egui_requested_repaint = false;
        if let Some(renderer) = &mut self.renderer
            && renderer.window().id() == window_id
        {
            let window = renderer.window.clone();
            #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
            renderer.process_accessibility_window_event(window.as_ref(), &event);
            let popup_was_open = egui::Popup::is_any_open(&renderer.egui_ctx);
            let response = renderer.egui_state.on_window_event(window.as_ref(), &event);
            // egui reports that RedrawRequested itself wants repainting. The
            // current event already satisfies that request, so scheduling it
            // again here would create a permanent redraw loop.
            if response.repaint && !is_redraw_event {
                egui_requested_repaint = true;
                window.request_redraw();
            }
            egui_consumed = response.consumed;
            egui_popup_open = widget_popup_owns_event(
                popup_was_open,
                egui::Popup::is_any_open(&renderer.egui_ctx),
            );
        }
        record_idle_event_attribution(
            self.performance_probe.as_mut(),
            is_own_window,
            is_redraw_event,
            egui_requested_repaint,
        );

        if egui_consumed {
            let application_must_handle = match &event {
                WindowEvent::CloseRequested
                | WindowEvent::DroppedFile(_)
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::Resized(_)
                | WindowEvent::Moved(_)
                | WindowEvent::ScaleFactorChanged { .. }
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
                    use winit::keyboard::{Key, NamedKey};
                    space_release_must_unwind(&event.logical_key, event.state, self.space_held)
                        || (event.state == winit::event::ElementState::Pressed
                            && escape_press_reaches_app(event.repeat, egui_popup_open)
                            && matches!(&event.logical_key, Key::Named(NamedKey::Escape))
                            && escape_action(self.escape_context()) != EscapeAction::None)
                        || (self.mosaic.is_active()
                            && event.state == winit::event::ElementState::Pressed
                            && matches!(
                                &event.logical_key,
                                Key::Named(
                                    NamedKey::Enter
                                        | NamedKey::ArrowLeft
                                        | NamedKey::ArrowRight
                                        | NamedKey::ArrowUp
                                        | NamedKey::ArrowDown
                                        | NamedKey::Home
                                        | NamedKey::End
                                        | NamedKey::PageUp
                                        | NamedKey::PageDown
                                )
                            ))
                        || (!application_shortcuts_blocked([
                            self.show_about,
                            self.show_update,
                            self.show_preferences,
                            self.show_file_associations,
                            self.pending_save.is_some(),
                            self.pending_rating_write.is_some(),
                            egui_popup_open,
                            self.context_menu_pos.is_some(),
                        ]) && route_consumed_keyboard_key_in_context(
                            &event.logical_key,
                            self.escape_context(),
                        ))
                }
                _ => false,
            };
            if !application_must_handle {
                return;
            }
        }

        if self.mosaic.is_active()
            && matches!(
                &event,
                WindowEvent::CursorMoved { .. }
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
            )
        {
            return;
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
            WindowEvent::CloseRequested => {
                if self.rating_write_worker.is_some() {
                    self.close_after_rating_write = true;
                    self.show_toast("Finishing the rating update before closing...");
                    return;
                }
                match close_disposition(self.save_job.is_some(), self.curation_worker.is_some()) {
                    CloseDisposition::Exit => event_loop.exit(),
                    CloseDisposition::WaitForSave => {
                        self.close_after_save = true;
                        self.show_toast("Finishing Save As before closing...");
                    }
                    CloseDisposition::WaitForCuration => {
                        if !self.close_after_curation {
                            let worker = self
                                .curation_worker
                                .as_ref()
                                .expect("active close disposition retains curation worker");
                            log::info!(
                                "close deferred for curation: operation={:?}, submitted={}",
                                worker.context.kind(),
                                worker.context.submitted()
                            );
                        }
                        self.close_after_curation = true;
                        self.request_redraw();
                    }
                    CloseDisposition::WaitForSaveAndCuration => {
                        self.close_after_save = true;
                        self.close_after_curation = true;
                        self.show_toast(
                            "Finishing Save As and the file operation before closing...",
                        );
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.open_path_request(path);
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
                                let scale = self
                                    .renderer
                                    .as_ref()
                                    .map_or(1.0, |renderer| renderer.window().scale_factor());
                                if scale.is_finite() && scale > 0.0 {
                                    self.context_menu_pos = Some([
                                        (self.cursor_pos.0 / scale) as f32,
                                        (self.cursor_pos.1 / scale) as f32,
                                    ]);
                                }
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
                    } else if self.space_held {
                        self.transform.is_panning = pressed;
                    } else {
                        self.transform.is_panning = false;
                    }
                } else if pressed
                    && matches!(
                        button,
                        winit::event::MouseButton::Back | winit::event::MouseButton::Forward
                    )
                {
                    let delta = if button == winit::event::MouseButton::Back {
                        -1
                    } else {
                        1
                    };
                    self.navigate(delta);
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
                        state,
                        logical_key,
                        repeat,
                        ..
                    },
                ..
            } => {
                use winit::keyboard::{Key, NamedKey};
                let pressed = state == winit::event::ElementState::Pressed;
                let is_space = is_space_key(&logical_key);
                let shortcuts_blocked = application_shortcuts_blocked([
                    self.show_about,
                    self.show_update,
                    self.show_preferences,
                    self.show_file_associations,
                    self.pending_save.is_some(),
                    self.pending_rating_write.is_some(),
                    egui_popup_open,
                    self.context_menu_pos.is_some(),
                ]);
                if is_space && !pressed {
                    if self.space_held {
                        self.space_held = false;
                        self.update_cursor_icon();
                        if space_tap_fits(self.space_dragged, shortcuts_blocked) {
                            self.fit_to_view();
                        }
                        self.space_dragged = false;
                    }
                    return;
                }
                if pressed
                    && escape_press_reaches_app(repeat, egui_popup_open)
                    && matches!(&logical_key, Key::Named(NamedKey::Escape))
                    && !self.show_about
                    && !self.show_update
                    && !self.show_preferences
                    && !self.show_file_associations
                    && self.pending_save.is_none()
                    && self.pending_rating_write.is_none()
                {
                    match escape_action(self.escape_context()) {
                        EscapeAction::CloseContextMenu => {
                            self.context_menu_pos = None;
                            self.request_redraw();
                            return;
                        }
                        EscapeAction::CancelCrop => {
                            self.cancel_crop();
                            return;
                        }
                        EscapeAction::LeaveHeal => {
                            self.toggle_heal_mode();
                            return;
                        }
                        EscapeAction::LeaveMosaic => {
                            self.leave_full_image_mosaic();
                            return;
                        }
                        EscapeAction::ClearRatingFilter => {
                            self.set_rating_filter(RatingFilter::All);
                            return;
                        }
                        EscapeAction::LeaveFullscreen => {
                            self.toggle_fullscreen();
                            return;
                        }
                        EscapeAction::None => {}
                    }
                }
                if shortcuts_blocked {
                    return;
                }
                // Space: hold = temporary hand tool; tap (no drag) = fit.
                if is_space {
                    if space_press_starts_hold(self.space_held) {
                        if self.heal.painting {
                            self.finish_heal_stroke();
                        }
                        self.space_held = true;
                        self.space_dragged = false;
                        self.update_cursor_icon();
                    }
                    return;
                }
                if !pressed {
                    return;
                }
                if repeat
                    && !repeated_viewer_action_allowed(&logical_key, self.transform.is_cropping)
                {
                    return;
                }
                if is_fullscreen_toggle_key(&logical_key, self.modifiers) {
                    self.toggle_fullscreen();
                    return;
                }
                if self.mosaic.is_active() {
                    match logical_key {
                        Key::Character(c)
                            if c.eq_ignore_ascii_case("g") && self.modifiers.shift_key() =>
                        {
                            self.leave_full_image_mosaic();
                        }
                        Key::Named(NamedKey::Enter | NamedKey::ArrowDown) => {
                            self.open_focused_mosaic_photo();
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            self.move_mosaic_focus(crate::mosaic::FocusDirection::Next);
                        }
                        Key::Named(NamedKey::ArrowLeft) => {
                            self.move_mosaic_focus(crate::mosaic::FocusDirection::Previous);
                        }
                        Key::Named(NamedKey::Home) => {
                            self.move_mosaic_focus(crate::mosaic::FocusDirection::First);
                        }
                        Key::Named(NamedKey::End) => {
                            self.move_mosaic_focus(crate::mosaic::FocusDirection::Last);
                        }
                        Key::Named(NamedKey::PageUp) => self.move_mosaic_page(-1),
                        Key::Named(NamedKey::PageDown) => self.move_mosaic_page(1),
                        _ => {}
                    }
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
                    Key::Named(NamedKey::ArrowUp) => self.toggle_full_image_mosaic(),
                    Key::Named(NamedKey::ArrowRight | NamedKey::PageDown) => self.navigate(1),
                    Key::Named(NamedKey::ArrowLeft | NamedKey::PageUp) => self.navigate(-1),
                    Key::Named(NamedKey::Home) => self.navigate(-999_999),
                    Key::Named(NamedKey::End) => self.navigate(999_999),
                    key if is_trash_shortcut_key(&key) => {
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
                    _ => {}
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                    renderer.window().request_redraw();
                }
                self.observe_current_display();
            }
            WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.observe_current_display();
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
                let playlist_pos = self.playlist.as_ref().and_then(Playlist::catalog_position);
                let rating = self.playlist.as_ref().map_or_else(
                    || crate::ui::RatingUiState {
                        state: self.presented_rating,
                        capability: self.current_rating_capability,
                        write_busy: self.rating_write_worker.is_some(),
                        recovery_unsettled: self.rating_recovery_unsettled,
                        discovery_busy: self.rating_scan_worker.is_some(),
                        pending_disclosure: self
                            .pending_rating_write
                            .as_ref()
                            .map(|pending| pending.assignment),
                        ..crate::ui::RatingUiState::default()
                    },
                    |playlist| crate::ui::RatingUiState {
                        state: self.presented_rating,
                        capability: self.current_rating_capability,
                        filter: playlist.filter(),
                        write_busy: self.rating_write_worker.is_some(),
                        recovery_unsettled: self.rating_recovery_unsettled,
                        discovery_busy: self.rating_scan_worker.is_some(),
                        outside_filter: playlist.outside_filter(),
                        visible_position: playlist
                            .visible_position()
                            .map(|position| (position.saturating_add(1), playlist.visible_len())),
                        match_count: playlist.visible_len(),
                        current_catalog_index: playlist.catalog_index(),
                        folder_count: playlist.files.len(),
                        pending_disclosure: self
                            .pending_rating_write
                            .as_ref()
                            .map(|pending| pending.assignment),
                    },
                );
                if self.show_filmstrip_panel && self.filmstrip_panel_open {
                    self.request_thumbs_for_filmstrip();
                }
                self.poll_thumbnails();
                let filmstrip = self.filmstrip_entries();
                let toast = self.toast.clone();
                let preview_kind = self.preview_job.as_ref().map(|job| job.context().kind);
                let is_opening = image_open_in_progress(self.session.is_loading(), preview_kind);
                let is_loading = self.session.is_loading() || preview_kind.is_some();
                let load_error = self.session.load_error.clone();
                let save_busy = self.save_job.is_some();
                let save_overwrite_pending = self.pending_save.is_some();
                let save_recovery_unsettled = self.save_recovery_unsettled;
                let crop_busy = self.crop_job.is_some();
                let crop_recovery_unsettled = self.crop_recovery_unsettled;
                let preview_recovery_unsettled = self.preview_recovery_unsettled;
                let preview_load_retry_blocked = self.preview_load_retry_blocked;
                let curation_status = self
                    .curation_worker
                    .as_ref()
                    .map(|worker| worker.status(self.close_after_curation));
                let curation_recovery_status = self.curation_recovery.status();
                let restore_recovery_unsettled =
                    self.curation_recovery.contains(CurationKind::Restore);
                let folder_scan_busy = self.folder_scan_job.is_some();
                let source_verification_busy = self.open_with_job.is_some();
                let path_str = self
                    .session
                    .presented_path
                    .as_deref()
                    .or(self.session.selected_path.as_deref())
                    .map(prefetch::privacy_safe_file_name);
                let selected_file_name = self
                    .session
                    .selected_path
                    .as_deref()
                    .map(prefetch::privacy_safe_file_name);
                let retain_exif = self.retain_exif;
                let theme_preference = self.theme_preference;
                let show_about = self.show_about;
                let show_update = self.show_update;
                let show_preferences = self.show_preferences;
                let show_file_associations = self.show_file_associations;
                let folder_sort = self.folder_sort;
                let external_edit_pending = self.external_edit_pending;
                let source_gone = self.source_gone;
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
                            can_previous: playback.can_step(-1),
                            can_next: playback.can_step(1),
                        });
                let pages = self.pages.as_ref().map(|cursor| crate::ui::PageUiInfo {
                    index: cursor.index(),
                    count: cursor.count(),
                    noun: cursor.kind().noun(),
                    can_previous: cursor.can_step(-1),
                    can_next: cursor.can_step(1),
                    visible_label: cursor.visible_copy(),
                    accessibility_label: cursor.accessibility_copy(),
                });
                let details = self.image_details.clone();
                let color_profile = self.current_image.as_ref().map(|image| image.color_profile);
                let is_cropping = self.transform.is_cropping;
                let crop_ratio = self.transform.crop_ratio;
                let custom_crop_ratio = self.custom_crop_ratio;
                let heal_busy = self.heal.is_busy();
                let heal_brush_radius = self.heal.brush_radius;
                let heal_feather_percent = self.heal.feather_percent;
                let heal_source = self
                    .heal
                    .refresh
                    .as_ref()
                    .map(|refresh| (refresh.candidate_index, refresh.candidate_count));
                let has_undo_edit = self.heal.history.can_undo();
                let has_redo_edit = self.heal.history.can_redo();
                let has_undo_trash = !self.last_trashed.is_empty();
                let has_unsaved_pixel_edits = self.unsaved_crop || has_undo_edit;
                let heal_painting = self.heal.painting;
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
                let dock = self.dock_input();
                let viewport_insets = crate::chrome::viewport_insets(
                    crate::chrome::DockViewModel::new(dock).layout(scale_factor),
                );
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
                let performance_image_path = self.session.presented_path.clone();
                if self.mosaic.is_active() {
                    self.sync_mosaic_gpu();
                }
                let (mosaic_draws, mosaic_ui) = self.mosaic_frame_geometry(scale_factor);

                let Some(renderer) = self.renderer.as_mut() else {
                    return;
                };

                if let Some(bg) = bg_override {
                    renderer.set_clear_color(bg);
                } else {
                    renderer.set_mode(theme_mode);
                }

                let placement = if mosaic_ui.is_none()
                    && let Some(size) = renderer.image_size()
                {
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
                let heal_supported =
                    image_is_fully_displayed(source_image_size, renderer.image_texture_size());
                let frame = crate::ui::UiFrameOwned {
                    dock,
                    retain_exif,
                    background_override: bg_override,
                    theme_preference,
                    theme_mode,
                    show_about,
                    show_update,
                    show_preferences,
                    show_file_associations,
                    folder_sort,
                    rating,
                    external_edit_pending,
                    source_gone,
                    file_path: path_str,
                    selected_file_name,
                    img_size,
                    animation,
                    pages,
                    details,
                    color_profile,
                    display_output: crate::display_state::output_status(
                        self.display_monitor.as_ref(),
                        self.display_hints,
                        self.display_profile_usable,
                    ),
                    is_cropping,
                    crop_ratio,
                    custom_crop_ratio,
                    heal_supported,
                    heal_busy,
                    heal_painting,
                    heal_brush_radius,
                    heal_feather_percent,
                    heal_source,
                    has_undo_edit,
                    has_redo_edit,
                    has_undo_trash,
                    has_unsaved_pixel_edits,
                    restore_recovery_unsettled,
                    is_panning,
                    space_held: self.space_held,
                    is_loading,
                    is_opening,
                    load_error,
                    save_busy,
                    save_overwrite_pending,
                    save_recovery_unsettled,
                    crop_busy,
                    crop_recovery_unsettled,
                    preview_recovery_unsettled,
                    preview_load_retry_blocked,
                    curation_status,
                    curation_recovery_status,
                    folder_scan_busy,
                    source_verification_busy,
                    playlist_pos,
                    pixel_scale: pixel_scale.unwrap_or(0.0),
                    toast,
                    mosaic: mosaic_ui,
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

                let presents_image = placement.is_some() || !mosaic_draws.is_empty();
                let frame_output =
                    renderer.render(placement, image_viewport, &mosaic_draws, |ui| {
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

                let mut save_overwrite_owns_dispatch = false;
                let mut rating_disclosure_owns_dispatch = false;
                let mut update_modal_owns_dispatch = false;
                let mut about_modal_owns_dispatch = false;
                let mut preferences_modal_owns_dispatch = false;
                let mut file_associations_modal_owns_dispatch = false;
                for action in ui_actions {
                    if !modal_dispatch_allows(
                        &mut save_overwrite_owns_dispatch,
                        self.pending_save.is_some(),
                        &action,
                        crate::ui::save_overwrite_action_allowed,
                    ) {
                        continue;
                    }
                    if !modal_dispatch_allows(
                        &mut rating_disclosure_owns_dispatch,
                        self.pending_rating_write.is_some(),
                        &action,
                        crate::ui::rating_disclosure_action_allowed,
                    ) {
                        continue;
                    }
                    if !modal_dispatch_allows(
                        &mut update_modal_owns_dispatch,
                        self.show_update,
                        &action,
                        crate::ui::update_modal_action_allowed,
                    ) {
                        continue;
                    }
                    if !modal_dispatch_allows(
                        &mut about_modal_owns_dispatch,
                        self.show_about,
                        &action,
                        crate::ui::about_modal_action_allowed,
                    ) {
                        continue;
                    }
                    if !modal_dispatch_allows(
                        &mut preferences_modal_owns_dispatch,
                        self.show_preferences,
                        &action,
                        crate::ui::preferences_modal_action_allowed,
                    ) {
                        continue;
                    }
                    if !modal_dispatch_allows(
                        &mut file_associations_modal_owns_dispatch,
                        self.show_file_associations,
                        &action,
                        crate::ui::file_associations_modal_action_allowed,
                    ) {
                        continue;
                    }
                    match action {
                        crate::ui::UiAction::Open => {
                            self.open_image_dialog();
                        }
                        crate::ui::UiAction::OpenFolder => self.open_folder_dialog(),
                        crate::ui::UiAction::Reload => self.reload_current_image(),
                        crate::ui::UiAction::OpenWith => self.open_current_with(),
                        crate::ui::UiAction::SaveAs => self.save_as(),
                        crate::ui::UiAction::ConfirmSaveOverwrite => {
                            self.confirm_save_overwrite();
                        }
                        crate::ui::UiAction::CancelSaveOverwrite => {
                            self.cancel_save_overwrite();
                        }
                        crate::ui::UiAction::Trash => {
                            self.trash_current();
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
                                log::warn!(
                                    "appearance preference save failed: {}",
                                    error.diagnostic_name()
                                );
                                self.show_toast(appearance_save_failure_message());
                            }
                        }
                        crate::ui::UiAction::SetFolderSort(sort) => {
                            self.set_folder_sort(sort);
                        }
                        crate::ui::UiAction::ShowAbout => {
                            self.show_about = true;
                            self.show_update = false;
                            self.show_preferences = false;
                            self.show_file_associations = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::CloseAbout => {
                            self.show_about = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::ShowUpdate => {
                            self.show_update = true;
                            self.show_about = false;
                            self.show_preferences = false;
                            self.show_file_associations = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::CloseUpdate => {
                            self.show_update = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::ShowPreferences => {
                            self.show_preferences = true;
                            self.show_about = false;
                            self.show_update = false;
                            self.show_file_associations = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::ClosePreferences => {
                            self.show_preferences = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::ShowFileAssociations => {
                            self.show_file_associations = true;
                            self.show_about = false;
                            self.show_update = false;
                            self.show_preferences = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::CloseFileAssociations => {
                            self.show_file_associations = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::AssignRating(assignment) => {
                            self.request_rating_assignment(assignment);
                        }
                        crate::ui::UiAction::SetRatingFilter(filter) => {
                            self.set_rating_filter(filter);
                        }
                        crate::ui::UiAction::ConfirmRatingDisclosure => {
                            self.confirm_rating_disclosure();
                        }
                        crate::ui::UiAction::CancelRatingDisclosure => {
                            self.cancel_rating_disclosure();
                        }
                        crate::ui::UiAction::ShowAllRatings => {
                            self.set_rating_filter(RatingFilter::All);
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
                        crate::ui::UiAction::StepSequence(delta) => {
                            self.step_sequence(delta);
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
                            self.toggle_fullscreen();
                        }
                        crate::ui::UiAction::ToggleMosaic => {
                            self.toggle_full_image_mosaic();
                        }
                        crate::ui::UiAction::OpenMosaicPhoto(index) => {
                            self.open_mosaic_photo(index);
                        }
                        crate::ui::UiAction::FitToView => self.fit_to_view(),
                        crate::ui::UiAction::ActualSize => self.set_actual_size(),
                        crate::ui::UiAction::ZoomIn => self.zoom_at_viewport_center(1.15),
                        crate::ui::UiAction::ZoomOut => {
                            self.zoom_at_viewport_center(1.0 / 1.15);
                        }
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
            UserEvent::OpenFile(path) => self.open_path_request(path),
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
        self.finish_open_with_check();
        self.poll_coherence_watch();
        self.poll_rating_write(event_loop);
        self.poll_rating_discovery();
        self.poll_curation_result(event_loop);
        self.poll_thumbnails();
        self.poll_prefetch();
        self.poll_heal_result();
        self.poll_preview_result();
        self.poll_auxiliary_load();
        self.poll_crop_result();
        self.poll_save_result(event_loop);
        self.poll_foreground_image_load();

        if self.poll_folder_scan()
            && let Some(renderer) = self.renderer.as_ref()
        {
            renderer.window().request_redraw();
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
                let egui_repaint_due = self.egui_repaint_at.is_some_and(|at| at <= deadline);
                if egui_repaint_due {
                    self.egui_repaint_at = None;
                    if let Some(probe) = self.performance_probe.as_mut()
                        && probe.idle_until.is_some()
                    {
                        probe.idle_scheduled_egui_repaints =
                            probe.idle_scheduled_egui_repaints.saturating_add(1);
                    }
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

fn bind_playlist_source_provenance(
    playlist: &mut Playlist,
    path: &Path,
    source: &crate::fs::ImageSource,
    require_existing_provenance: bool,
) -> bool {
    if require_existing_provenance && playlist.scan_provenance(path).is_none() {
        return false;
    }
    let Some(provenance) = source.scan_provenance() else {
        return false;
    };
    playlist.set_scan_provenance(path, Some(provenance))
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

fn performance_ui_is_settled(egui_repaint_at: Option<Instant>) -> bool {
    egui_repaint_at.is_none()
}

fn idle_window_state(renderer: Option<&Renderer>) -> (bool, bool) {
    renderer.map_or((false, false), |renderer| {
        (
            renderer.window().has_focus(),
            renderer
                .egui_ctx
                .input(|input| input.pointer.hover_pos().is_some()),
        )
    })
}

fn record_idle_event_attribution(
    probe: Option<&mut PerformanceProbe>,
    is_own_window: bool,
    is_redraw_event: bool,
    event_requested_repaint: bool,
) {
    if !is_own_window || is_redraw_event {
        return;
    }
    let Some(probe) = probe else {
        return;
    };
    if probe.idle_until.is_none() {
        return;
    }
    probe.idle_non_redraw_events = probe.idle_non_redraw_events.saturating_add(1);
    if event_requested_repaint {
        probe.idle_event_repaint_requests = probe.idle_event_repaint_requests.saturating_add(1);
    }
}

fn present_image_patch(
    renderer: Option<&mut Renderer>,
    image: &DecodedImage,
    patch: &crate::heal::ImagePatch,
) -> Result<(), String> {
    let renderer = renderer.ok_or_else(|| "renderer is unavailable".to_owned())?;
    let patch_presented = renderer.update_image_patch(patch);
    complete_patch_presentation(patch_presented, || {
        renderer
            .set_image(image, None)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

fn complete_patch_presentation<E>(
    patch_presented: bool,
    full_image_fallback: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    if patch_presented {
        Ok(())
    } else {
        full_image_fallback()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn source_change_cancels_pending_overwrite_without_touching_destination() {
        let workspace = crate::ephemeral::TempWorkspace::new("cancel_pending_save").unwrap();
        let destination_path = workspace.path().join("existing.png");
        let original = b"existing destination";
        std::fs::write(&destination_path, original).unwrap();
        let mut pending_save = Some(PendingSave {
            source_path: workspace.path().join("source.png"),
            source_image: Arc::new(DecodedImage {
                rgba: vec![10, 20, 30, 255],
                width: 1,
                height: 1,
                color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
                working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
            }),
            source: None,
            destination: crate::edit::prepare_save_destination(&destination_path).unwrap(),
            pixel_transform: crate::edit::PixelTransform::default(),
            options: crate::edit::SaveOptions::default(),
        });

        assert!(cancel_pending_save_for_source_change(&mut pending_save));
        assert!(pending_save.is_none());
        assert_eq!(std::fs::read(&destination_path).unwrap(), original);
        assert!(!cancel_pending_save_for_source_change(&mut pending_save));
    }

    #[test]
    fn source_change_cancels_pending_rating_disclosure() {
        let mut pending_rating_write = Some(PendingRatingWrite {
            path: PathBuf::from("selected.jpg"),
            assignment: RatingAssignment::Set(
                crate::ratings::Rating::new(4).expect("valid test rating"),
            ),
        });

        assert!(cancel_pending_rating_for_source_change(
            &mut pending_rating_write
        ));
        assert!(pending_rating_write.is_none());
        assert!(!cancel_pending_rating_for_source_change(
            &mut pending_rating_write
        ));
    }

    #[test]
    fn every_filter_result_that_can_replace_or_clear_source_revokes_consent() {
        assert!(!filter_selection_changes_source(
            FilterSelection::Stay,
            true
        ));
        assert!(filter_selection_changes_source(
            FilterSelection::Stay,
            false
        ));
        assert!(filter_selection_changes_source(
            FilterSelection::Select(4),
            true
        ));
        assert!(filter_selection_changes_source(
            FilterSelection::Empty,
            true
        ));
    }

    #[test]
    fn overwrite_dispatch_ownership_is_sticky_when_an_action_opens_the_modal() {
        let mut owns_dispatch = false;

        assert!(modal_dispatch_allows(
            &mut owns_dispatch,
            false,
            &crate::ui::UiAction::SaveAs,
            crate::ui::save_overwrite_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut owns_dispatch,
            true,
            &crate::ui::UiAction::Trash,
            crate::ui::save_overwrite_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut owns_dispatch,
            true,
            &crate::ui::UiAction::CancelSaveOverwrite,
            crate::ui::save_overwrite_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut owns_dispatch,
            false,
            &crate::ui::UiAction::Open,
            crate::ui::save_overwrite_action_allowed,
        ));
    }

    #[test]
    fn rating_dispatch_ownership_is_sticky_when_an_action_opens_the_modal() {
        let mut owns_dispatch = false;

        assert!(modal_dispatch_allows(
            &mut owns_dispatch,
            false,
            &crate::ui::UiAction::AssignRating(RatingAssignment::Set(
                crate::ratings::Rating::new(4).expect("valid test rating")
            )),
            crate::ui::rating_disclosure_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut owns_dispatch,
            true,
            &crate::ui::UiAction::Trash,
            crate::ui::rating_disclosure_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut owns_dispatch,
            true,
            &crate::ui::UiAction::CancelRatingDisclosure,
            crate::ui::rating_disclosure_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut owns_dispatch,
            false,
            &crate::ui::UiAction::Open,
            crate::ui::rating_disclosure_action_allowed,
        ));
    }

    #[test]
    fn informational_modal_dispatch_ownership_is_sticky() {
        let mut update_owns_dispatch = false;
        assert!(modal_dispatch_allows(
            &mut update_owns_dispatch,
            false,
            &crate::ui::UiAction::ShowUpdate,
            crate::ui::update_modal_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut update_owns_dispatch,
            true,
            &crate::ui::UiAction::Trash,
            crate::ui::update_modal_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut update_owns_dispatch,
            true,
            &crate::ui::UiAction::CloseUpdate,
            crate::ui::update_modal_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut update_owns_dispatch,
            false,
            &crate::ui::UiAction::Open,
            crate::ui::update_modal_action_allowed,
        ));

        let mut about_owns_dispatch = false;
        assert!(modal_dispatch_allows(
            &mut about_owns_dispatch,
            false,
            &crate::ui::UiAction::ShowAbout,
            crate::ui::about_modal_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut about_owns_dispatch,
            true,
            &crate::ui::UiAction::SaveAs,
            crate::ui::about_modal_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut about_owns_dispatch,
            true,
            &crate::ui::UiAction::CloseAbout,
            crate::ui::about_modal_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut about_owns_dispatch,
            false,
            &crate::ui::UiAction::OpenFolder,
            crate::ui::about_modal_action_allowed,
        ));

        let mut preferences_owns_dispatch = false;
        assert!(modal_dispatch_allows(
            &mut preferences_owns_dispatch,
            false,
            &crate::ui::UiAction::ShowPreferences,
            crate::ui::preferences_modal_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut preferences_owns_dispatch,
            true,
            &crate::ui::UiAction::Trash,
            crate::ui::preferences_modal_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut preferences_owns_dispatch,
            true,
            &crate::ui::UiAction::SetFolderSort(crate::fs::FolderSort::Name),
            crate::ui::preferences_modal_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut preferences_owns_dispatch,
            true,
            &crate::ui::UiAction::ClosePreferences,
            crate::ui::preferences_modal_action_allowed,
        ));

        let mut associations_owns_dispatch = false;
        assert!(modal_dispatch_allows(
            &mut associations_owns_dispatch,
            false,
            &crate::ui::UiAction::ShowFileAssociations,
            crate::ui::file_associations_modal_action_allowed,
        ));
        assert!(!modal_dispatch_allows(
            &mut associations_owns_dispatch,
            true,
            &crate::ui::UiAction::Trash,
            crate::ui::file_associations_modal_action_allowed,
        ));
        assert!(modal_dispatch_allows(
            &mut associations_owns_dispatch,
            true,
            &crate::ui::UiAction::CloseFileAssociations,
            crate::ui::file_associations_modal_action_allowed,
        ));
    }

    fn assert_selected_replacement_stays_bound_to_accepted_source(
        fixture: &str,
        require_existing_provenance: bool,
    ) {
        let workspace = crate::ephemeral::TempWorkspace::new(fixture).unwrap();
        let path = workspace.path().join("selected.png");
        let original = workspace.path().join("original.png");
        let replacement = workspace.path().join("replacement.png");
        image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]))
            .save(&path)
            .unwrap();
        image::RgbImage::from_pixel(1, 1, image::Rgb([0, 0, 255]))
            .save(&replacement)
            .unwrap();
        let accepted = crate::fs::ImageSource::open_regular(&path).unwrap();
        std::fs::rename(&path, &original).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        let replacement_source = crate::fs::ImageSource::open_regular(&path).unwrap();
        let mut playlist = Playlist::new(vec![path.clone()], 0);
        playlist.set_scan_provenance(&path, replacement_source.scan_provenance());

        assert!(bind_playlist_source_provenance(
            &mut playlist,
            &path,
            &accepted,
            require_existing_provenance,
        ));

        let provenance = playlist.scan_provenance(&path).unwrap();
        assert!(crate::fs::ImageSource::open_scanned(&path, provenance).is_err());
    }

    #[test]
    fn scan_before_display_preserves_the_explicitly_accepted_source() {
        assert_selected_replacement_stays_bound_to_accepted_source(
            "selected_scan_before_display",
            true,
        );
    }

    #[test]
    fn display_before_scan_preserves_the_explicitly_accepted_source() {
        assert_selected_replacement_stays_bound_to_accepted_source(
            "selected_display_before_scan",
            false,
        );
    }

    #[test]
    fn bare_digits_assign_ratings_and_repeat_never_writes() {
        assert_eq!(
            rating_assignment_for_key("0", false),
            Some(RatingAssignment::Clear)
        );
        for value in 1_u8..=5 {
            assert_eq!(
                rating_assignment_for_key(&value.to_string(), false),
                Some(RatingAssignment::Set(
                    crate::ratings::Rating::new(value).unwrap()
                ))
            );
            assert_eq!(rating_assignment_for_key(&value.to_string(), true), None);
        }
        assert_eq!(rating_assignment_for_key("6", false), None);
        assert_eq!(rating_assignment_for_key("1", true), None);
    }

    #[test]
    fn consumed_numeric_input_is_not_forced_into_rating_shortcuts() {
        for key in ["0", "1", "2", "3", "4", "5"] {
            assert!(!route_consumed_keyboard_key(
                &winit::keyboard::Key::Character(key.into()),
                false,
                false,
                false,
            ));
        }
    }

    #[test]
    fn menus_modals_and_popups_own_keyboard_shortcuts() {
        assert!(!application_shortcuts_blocked([false; 7]));
        for owner in 0..7 {
            let mut owners = [false; 7];
            owners[owner] = true;
            assert!(application_shortcuts_blocked(owners));
        }
    }

    #[test]
    fn held_space_release_unwinds_even_when_an_overlay_takes_shortcuts() {
        use winit::event::ElementState;
        use winit::keyboard::{Key, NamedKey};

        let space = Key::Named(NamedKey::Space);
        assert!(space_release_must_unwind(
            &space,
            ElementState::Released,
            true
        ));
        assert!(!space_release_must_unwind(
            &space,
            ElementState::Released,
            false
        ));
        assert!(!space_release_must_unwind(
            &space,
            ElementState::Pressed,
            true
        ));
    }

    #[test]
    fn filmstrip_layout_uses_the_projected_entry_count() {
        assert!(!filmstrip_is_available(0));
        assert!(!filmstrip_is_available(1));
        assert!(filmstrip_is_available(2));
    }

    #[test]
    fn curation_worker_dispatches_without_waiting_and_wakes_once() {
        let (started_sender, started_receiver) = mpsc::sync_channel(1);
        let (release_sender, release_receiver) = mpsc::sync_channel(1);
        let (wake_sender, wake_receiver) = mpsc::sync_channel(1);
        let (result_receiver, join) = spawn_curation_thread(
            "viewr-test-curation",
            move || {
                started_sender.send(()).unwrap();
                release_receiver.recv().unwrap();
                17_u8
            },
            move || wake_sender.send(()).unwrap(),
        )
        .expect("test worker should spawn");

        started_receiver.recv().unwrap();
        assert!(matches!(poll_worker(&result_receiver), WorkerPoll::Pending));
        release_sender.send(()).unwrap();
        wake_receiver.recv().unwrap();
        assert!(matches!(
            poll_worker(&result_receiver),
            WorkerPoll::Ready(17)
        ));
        join.join().unwrap();
        assert!(matches!(
            wake_receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn curation_worker_panic_disconnects_and_wakes_once() {
        let (wake_sender, wake_receiver) = mpsc::sync_channel(1);
        let (wake_release_sender, wake_release_receiver) = mpsc::sync_channel(1);
        let (result_receiver, join) = spawn_curation_thread::<u8>(
            "viewr-test-curation-panic",
            || panic!("controlled curation worker panic"),
            move || {
                wake_sender.send(()).unwrap();
                wake_release_receiver.recv().unwrap();
            },
        )
        .expect("test worker should spawn");

        wake_receiver.recv().unwrap();
        assert!(matches!(
            poll_worker(&result_receiver),
            WorkerPoll::Disconnected
        ));
        wake_release_sender.send(()).unwrap();
        assert!(join.join().is_err());
        assert!(matches!(
            wake_receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn preserved_undo_indices_follow_same_scope_nonundoable_removals() {
        let workspace = crate::ephemeral::TempWorkspace::new("preserved_undo_indices").unwrap();
        let source_path = workspace.path().join("source.png");
        std::fs::write(&source_path, b"source").unwrap();
        let source = Arc::new(crate::fs::ImageSource::open(&source_path).unwrap());
        let record = |name: &str, playlist_index| TrashedFile {
            receipt: crate::curate::TrashReceipt::for_test(
                PathBuf::from(name),
                Arc::clone(&source),
            ),
            playlist_index,
        };
        let scope = Arc::new(PlaylistScope);
        let other_scope = Arc::new(PlaylistScope);
        let mut records = vec![record("b.png", 1), record("d.png", 3)];

        rebase_preserved_trash_action(&mut records, Some(&scope), Some(&scope), &[0]);
        assert_eq!(
            records
                .iter()
                .map(|record| record.playlist_index)
                .collect::<Vec<_>>(),
            vec![0, 2],
            "a permanent delete before the pending action shifts both receipts"
        );
        rebase_preserved_trash_action(&mut records, Some(&scope), Some(&scope), &[0]);
        assert_eq!(
            records
                .iter()
                .map(|record| record.playlist_index)
                .collect::<Vec<_>>(),
            vec![0, 1],
            "a later receiptless Trash move before the trailing receipt shifts it"
        );

        let mut simultaneous = vec![record("b.png", 1), record("d.png", 3)];
        rebase_preserved_trash_action(&mut simultaneous, Some(&scope), Some(&scope), &[0, 2]);
        assert_eq!(
            simultaneous
                .iter()
                .map(|record| record.playlist_index)
                .collect::<Vec<_>>(),
            vec![0, 2],
            "a trailing current item remains after the pending trailing receipt"
        );

        let mut unrelated = vec![record("b.png", 1)];
        rebase_preserved_trash_action(&mut unrelated, Some(&scope), Some(&other_scope), &[0]);
        assert_eq!(unrelated[0].playlist_index, 1);
    }

    #[test]
    fn heal_presentation_failure_keeps_app_state_and_routes_safe_retry_copy() {
        let mut image = DecodedImage {
            rgba: [18, 36, 54, 255].repeat(16 * 16),
            width: 16,
            height: 16,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
        };
        let job = crate::heal::SpotHealJob::prepare(
            &image,
            &[crate::heal::StrokePoint { x: 8.0, y: 8.0 }],
            crate::heal::MIN_BRUSH_RADIUS,
        )
        .unwrap()
        .unwrap();
        let mut refresh = Some(HealRefresh {
            job,
            candidate_index: 1,
            candidate_count: 3,
        });
        let mut history = crate::heal::PatchHistory::new(64);
        let original_pixels = image.rgba.clone();
        let secret = "C:\\private\\album\\bad\n\u{202e}.png";
        let error = commit_presented_heal(
            &mut image,
            &mut history,
            &mut refresh,
            &crate::heal::SpotHealResult {
                patch: crate::heal::ImagePatch {
                    bounds: crate::edit::Rect {
                        x: 2,
                        y: 3,
                        width: 1,
                        height: 1,
                    },
                    rgba: vec![250, 1, 2, 255],
                },
                candidate_index: 2,
                candidate_count: 4,
            },
            None,
            true,
            |_, _| complete_patch_presentation(false, || Err(secret)),
        )
        .expect_err("the injected full-image fallback must fail");

        assert_eq!(image.rgba, original_pixels);
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        let refresh = refresh.expect("the previous refresh must remain available");
        assert_eq!(refresh.candidate_index, 1);
        assert_eq!(refresh.candidate_count, 3);

        let message = edit_transaction_failure_message("Spot heal", &error, false);
        assert!(message.ends_with("Try again."));
        assert!(!message.contains("Spot healed"));
        assert!(!message.contains("private"));
        assert!(!message.contains('\n'));
        assert!(!message.contains('\u{202e}'));
    }

    #[test]
    fn spot_heal_success_copy_names_memory_and_save_as_boundaries() {
        let applied = heal_success_message(false, 0, 4);
        assert!(applied.contains("in memory"));
        assert!(applied.contains("Save As"));
        assert!(applied.contains("Undo"));
        assert_eq!(heal_success_message(true, 2, 4), "Heal source 3 of 4");
    }

    #[test]
    fn save_success_copy_confirms_when_edited_pixels_were_exported() {
        assert_eq!(
            save_success_message(crate::edit::MetadataDisposition::Stripped, true),
            "Saved edited copy · metadata stripped"
        );
        assert_eq!(
            save_success_message(crate::edit::MetadataDisposition::Retained, false),
            "Saved copy · EXIF retained"
        );
    }

    #[test]
    fn preference_recovery_notice_combines_independent_fallbacks() {
        assert_eq!(startup_preference_recovery_notice(None, None), None);
        assert_eq!(
            startup_preference_recovery_notice(Some(PreferenceRecovery::Invalid), None),
            Some("Could not restore saved appearance. Using System.")
        );
        assert_eq!(
            startup_preference_recovery_notice(
                None,
                Some(crate::folder_sort_preference::Recovery::Oversized)
            ),
            Some("Could not restore saved folder sort. Using Latest First.")
        );
        assert_eq!(
            startup_preference_recovery_notice(
                Some(PreferenceRecovery::Unreadable),
                Some(crate::folder_sort_preference::Recovery::Invalid)
            ),
            Some(
                "Could not restore saved appearance or folder sort. Using System and Latest First."
            )
        );
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
    fn crop_recovery_preserves_every_auxiliary_job_terminal_state() {
        let path = PathBuf::from("source.png");
        let source_image = Arc::new(DecodedImage {
            rgba: vec![10, 20, 30, 255],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
        });
        let transform = Transform {
            crop_start: Some((0.2, 0.3)),
            ..Transform::default()
        };
        let context = || AuxiliaryLoadContext {
            path: path.clone(),
            generation: 42,
        };
        let recovery = |auxiliary_job| CropRecovery {
            source_path: path.clone(),
            source_generation: 42,
            source_image: Arc::clone(&source_image),
            transform,
            animation: None,
            pages: None,
            auxiliary_job,
        };
        let output = || {
            (
                Ok(AuxiliarySequence::None),
                crate::image_info::ImageDetails::default(),
                RatingObservation {
                    state: RatingState::Unrated,
                    capability: RatingWriteCapability::ReadOnlyFormat,
                },
            )
        };

        let (pending_completion, pending_job) = OneShotJob::new(context(), || {});
        let pending = recovery(Some(pending_job)).into_restored_edit_state();
        assert!(matches!(
            pending.auxiliary_job.as_ref().unwrap().poll(),
            JobPoll::Pending
        ));
        assert_eq!(pending.transform.crop_start, None);
        drop(pending_completion);

        let (ready_completion, ready_job) = OneShotJob::new(context(), || {});
        assert!(ready_completion.complete(output()));
        let ready = recovery(Some(ready_job)).into_restored_edit_state();
        assert!(matches!(
            ready.auxiliary_job.as_ref().unwrap().poll(),
            JobPoll::Ready(_)
        ));

        let (disconnected_completion, disconnected_job) = OneShotJob::new(context(), || {});
        drop(disconnected_completion);
        let disconnected = recovery(Some(disconnected_job)).into_restored_edit_state();
        assert!(matches!(
            disconnected.auxiliary_job.as_ref().unwrap().poll(),
            JobPoll::Disconnected
        ));
    }

    #[test]
    fn worker_poll_distinguishes_pending_ready_and_disconnected() {
        let (sender, receiver) = mpsc::channel();
        assert!(matches!(poll_worker::<u8>(&receiver), WorkerPoll::Pending));
        sender.send(7).unwrap();
        assert!(matches!(poll_worker(&receiver), WorkerPoll::Ready(7)));
        drop(sender);
        assert!(matches!(
            poll_worker::<u8>(&receiver),
            WorkerPoll::Disconnected
        ));
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
    fn trash_undo_scope_requires_the_exact_playlist_instance() {
        let original = Arc::new(PlaylistScope);
        let same = Arc::clone(&original);
        let replacement = Arc::new(PlaylistScope);
        let playlist = Playlist::new(vec![PathBuf::from("active.jpg")], 0);

        assert!(restore_targets_active_playlist(
            Some(&playlist),
            Some(&same),
            Some(&original)
        ));
        assert!(!restore_targets_active_playlist(
            Some(&playlist),
            Some(&replacement),
            Some(&original)
        ));
        assert!(!restore_targets_active_playlist(
            Some(&playlist),
            Some(&original),
            None
        ));
        assert!(!restore_targets_active_playlist(
            Some(&playlist),
            None,
            Some(&original)
        ));
        assert!(!restore_targets_active_playlist(
            None,
            Some(&same),
            Some(&original)
        ));
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
    fn delayed_egui_repaint_is_not_settled_idle() {
        assert!(!performance_ui_is_settled(Some(Instant::now())));
        assert!(performance_ui_is_settled(None));
    }

    #[test]
    fn idle_event_attribution_counts_only_own_non_redraw_events() {
        let mut probe = PerformanceProbe::new(Instant::now());
        probe.idle_until = Some(Instant::now());

        record_idle_event_attribution(Some(&mut probe), true, false, false);
        record_idle_event_attribution(Some(&mut probe), true, false, true);
        record_idle_event_attribution(Some(&mut probe), true, true, true);
        record_idle_event_attribution(Some(&mut probe), false, false, true);
        record_idle_event_attribution(None, true, false, true);

        assert_eq!(probe.idle_non_redraw_events, 2);
        assert_eq!(probe.idle_event_repaint_requests, 1);

        probe.idle_until = None;
        record_idle_event_attribution(Some(&mut probe), true, false, true);
        assert_eq!(probe.idle_non_redraw_events, 2);
        assert_eq!(probe.idle_event_repaint_requests, 1);
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
}
