//! The application: a message loop of our own on winit's event loop. For Phase 0
//! it opens a window, sets up the GPU renderer, and clears each frame to the
//! theme background. The Elm-style shape (one state, messages, update, render)
//! is borrowed without depending on a UI framework.
#![allow(unsafe_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
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
use crate::playlist::{FilterSelection, Playlist, ScanPurpose};
use crate::ratings::{
    RatingAssignment, RatingFilter, RatingObservation, RatingState, RatingWriteCapability,
    RatingWriteError,
};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

use crate::curate::{GuardedActionError, TrashRestoreDisposition, TrashedFile};
use crate::curation_state::{
    CurationCloseDisposition, CurationKind, CurationRecovery, CurationTerminalState,
    curation_close_disposition, curation_recovery_message, curation_status, file_count,
};
use crate::decode::{DecodedImage, LoadedImage};
use crate::error::Error;
use crate::gpu::{FrameResult, ImagePreview, Renderer};
use crate::job::{JobPoll, OneShotJob};
use crate::prefetch::{self, PrefetchCache};
use crate::presentation::{
    ImageReuseEligibility, NavigationImagePlan, PresentationKind, PresentedFrameTransition,
    durable_presentation_error, external_edit_pending_after_frame_transition,
    image_open_in_progress, navigation_image_plan, preview_job_matches,
};
use crate::theme::{Preference, PreferenceRecovery};
use crate::thumbs::{self, ThumbnailCompletion};
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

#[allow(
    clippy::too_many_lines,
    reason = "startup assembles the complete explicit application state in one place"
)]
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
    let mut app = App {
        session: crate::session::Session {
            selected_path: image_path,
            ..Default::default()
        },
        renderer: None,
        playlist: None,
        playlist_scope: None,
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
        is_fullscreen: false,
        last_trashed: Vec::new(),
        last_trashed_scope: None,
        current_image: None,
        current_source: None,
        current_image_reuse: ImageReuseEligibility::Ineligible,
        animation: None,
        image_details: None,
        auxiliary_job: None,
        #[cfg(target_os = "windows")]
        open_with_job: None,
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
        appearance_recovery,
        show_about: false,
        show_update: false,
        external_edit_pending: false,
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
    };
    if let Some(path) = app.session.selected_path.clone() {
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

fn appearance_save_failure_message() -> &'static str {
    "Appearance changed for this session but could not be remembered. Check local configuration storage, then choose it again."
}

fn rating_write_failure_message(error: RatingWriteError) -> &'static str {
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

struct RatingWriteWorker {
    path: PathBuf,
    assignment: RatingAssignment,
    result_rx: Receiver<Result<crate::ratings::VerifiedRatingWrite, RatingWriteError>>,
    join: JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RatingDiscoveryTransition {
    Apply,
    Start,
    KeepRunning,
    CancelAndApply,
}

/// Exact identity of one installed playlist. Restores may rejoin only this view.
#[derive(Debug)]
struct PlaylistScope;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseDisposition {
    Exit,
    WaitForSave,
    WaitForCuration,
    WaitForSaveAndCuration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveTerminalState {
    Succeeded,
    Failed,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveCloseDisposition {
    StayOpen,
    Exit,
    WaitForCuration,
    CancelDeferredClose,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveStartBlocker {
    Recovery,
    FolderOpen,
    RatingWrite,
    Preview,
    SpotHeal,
    Crop,
    Save,
}

const fn close_disposition(save_active: bool, curation_active: bool) -> CloseDisposition {
    match (save_active, curation_active) {
        (false, false) => CloseDisposition::Exit,
        (true, false) => CloseDisposition::WaitForSave,
        (false, true) => CloseDisposition::WaitForCuration,
        (true, true) => CloseDisposition::WaitForSaveAndCuration,
    }
}

const fn save_close_disposition(
    close_requested: bool,
    terminal: SaveTerminalState,
    curation_active: bool,
) -> SaveCloseDisposition {
    if !close_requested {
        SaveCloseDisposition::StayOpen
    } else if !matches!(terminal, SaveTerminalState::Succeeded) {
        SaveCloseDisposition::CancelDeferredClose
    } else if curation_active {
        SaveCloseDisposition::WaitForCuration
    } else {
        SaveCloseDisposition::Exit
    }
}

fn save_start_blocker<const N: usize>(
    blockers: [Option<SaveStartBlocker>; N],
) -> Option<SaveStartBlocker> {
    blockers.into_iter().flatten().next()
}

const fn save_start_blocker_message(blocker: SaveStartBlocker) -> &'static str {
    match blocker {
        SaveStartBlocker::Recovery => crate::ui::SAVE_RECOVERY_STATUS,
        SaveStartBlocker::FolderOpen => {
            "Wait for the selected folder to finish opening before saving a copy"
        }
        SaveStartBlocker::RatingWrite => {
            "Wait for the rating update to finish before saving a copy"
        }
        SaveStartBlocker::Preview => "Wait for the image preview to finish before saving",
        SaveStartBlocker::SpotHeal => "Wait for spot heal to finish before saving",
        SaveStartBlocker::Crop => "Wait for the crop to finish before saving",
        SaveStartBlocker::Save => "A copy is already being saved",
    }
}

impl CurationKind {
    const fn work(self) -> CurrentWork {
        match self {
            Self::Trash => CurrentWork::TrashMove,
            Self::PermanentDelete => CurrentWork::PermanentDelete,
            Self::Restore => CurrentWork::TrashRestore,
        }
    }
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

#[cfg(target_os = "windows")]
struct OpenWithContext {
    path: PathBuf,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

type AuxiliaryLoadResult = (
    Result<Option<crate::animated::DecodedAnimation>, String>,
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

const fn folder_scan_blocks_save(purpose: Option<&ScanPurpose>) -> bool {
    matches!(purpose, Some(ScanPurpose::OpenFolder))
}

const fn filter_selection_changes_source(
    selection: FilterSelection,
    has_current_image: bool,
) -> bool {
    match selection {
        FilterSelection::Stay => !has_current_image,
        FilterSelection::Select(_) | FilterSelection::Empty => true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresentedRatingTransition {
    Retain,
    Replace(RatingState),
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RatingRecoveryTransition {
    Retain,
    MarkUnsettled,
    AcceptSource,
}

struct CropRecovery {
    source_path: PathBuf,
    source_generation: u64,
    source_image: Arc<DecodedImage>,
    transform: Transform,
    animation: Option<crate::animated::AnimationPlayback>,
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
}

struct RestoredCropEditState {
    transform: Transform,
    animation: Option<crate::animated::AnimationPlayback>,
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
}

impl CropRecovery {
    fn into_restored_edit_state(self) -> RestoredCropEditState {
        let Self {
            mut transform,
            animation,
            auxiliary_job,
            ..
        } = self;
        transform.crop_start = None;
        RestoredCropEditState {
            transform,
            animation,
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
const PERMANENT_DELETE_ACTION: &str = "Delete permanently";

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CurrentWork {
    TrashMove,
    PermanentDelete,
    TrashRestore,
    #[cfg(target_os = "windows")]
    SourceVerification,
    FolderScan,
    ImagePreparation,
    Crop,
    Save,
    SpotHeal,
    RatingWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuardedActionKind {
    Trash,
    PermanentDelete,
}

fn guarded_action_failure_message(action: GuardedActionKind, error: &GuardedActionError) -> String {
    match (action, error) {
        (GuardedActionKind::Trash, GuardedActionError::Changed) =>
            "This file changed after it was displayed. Reload it before moving it to Trash. Nothing was moved."
                .to_owned(),
        (GuardedActionKind::PermanentDelete, GuardedActionError::Changed) =>
            "This file changed after it was displayed. Reload it before deleting it. Nothing was deleted."
                .to_owned(),
        (GuardedActionKind::Trash, GuardedActionError::Missing) =>
            "This file is no longer available. Nothing was moved.".to_owned(),
        (GuardedActionKind::PermanentDelete, GuardedActionError::Missing) =>
            "This file is no longer available. Nothing was deleted.".to_owned(),
        (GuardedActionKind::Trash, GuardedActionError::Unsupported) =>
            "This filesystem entry cannot be safely moved from the displayed source. Nothing was moved."
                .to_owned(),
        (GuardedActionKind::PermanentDelete, GuardedActionError::Unsupported) =>
            "This filesystem entry cannot be safely deleted from the displayed source. Nothing was deleted."
                .to_owned(),
        (GuardedActionKind::Trash, GuardedActionError::Unavailable) =>
            "Safe file identity could not be verified. Nothing was moved.".to_owned(),
        (GuardedActionKind::PermanentDelete, GuardedActionError::Unavailable) =>
            "Safe file identity could not be verified. Nothing was deleted.".to_owned(),
        (GuardedActionKind::Trash, GuardedActionError::OperationFailed(error)) => {
            format!("Trash failed: {error}. Nothing was moved.")
        }
        (GuardedActionKind::PermanentDelete, GuardedActionError::OperationFailed(error)) => {
            format!("Delete failed: {error}. Nothing was deleted.")
        }
    }
}

fn log_guarded_action_failure(action: GuardedActionKind, error: &GuardedActionError) {
    let action = match action {
        GuardedActionKind::Trash => "trash",
        GuardedActionKind::PermanentDelete => "permanent_delete",
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

fn image_preparation_work(foreground_load: bool, preview_preparation: bool) -> Option<CurrentWork> {
    (foreground_load || preview_preparation).then_some(CurrentWork::ImagePreparation)
}

fn crop_work(selection_active: bool, worker_active: bool) -> Option<CurrentWork> {
    (selection_active || worker_active).then_some(CurrentWork::Crop)
}

fn current_work_blocker<const N: usize>(work: [Option<CurrentWork>; N]) -> Option<CurrentWork> {
    work.into_iter().flatten().next()
}

fn blocked_action_message(action: &str, blocker: CurrentWork) -> String {
    let work = match blocker {
        CurrentWork::TrashMove => "the move to Trash",
        CurrentWork::PermanentDelete => "the permanent delete",
        CurrentWork::TrashRestore => "the Trash restore",
        #[cfg(target_os = "windows")]
        CurrentWork::SourceVerification => "source verification",
        CurrentWork::FolderScan => "the folder scan",
        CurrentWork::ImagePreparation => "image preparation",
        CurrentWork::Crop => "the crop",
        CurrentWork::Save => "Save As",
        CurrentWork::SpotHeal => "Spot Heal",
        CurrentWork::RatingWrite => "the rating update",
    };
    format!("Wait for {work} to finish before {action}")
}

fn curation_action_preflight(
    active: Option<CurationKind>,
    has_work: bool,
    action: &str,
    empty_message: &str,
) -> Option<String> {
    if let Some(kind) = active {
        Some(blocked_action_message(action, kind.work()))
    } else if has_work {
        None
    } else {
        Some(empty_message.to_owned())
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

fn edit_transaction_failure_message<E>(
    action: &str,
    error: &crate::heal::PatchPresentationError<E>,
    reloading_source: bool,
) -> String {
    match error {
        crate::heal::PatchPresentationError::Edit(_) => {
            format!("{action} could not be applied. The image and edit history are unchanged.")
        }
        crate::heal::PatchPresentationError::Presentation(_) => {
            format!("{action} was not applied because the display could not update. Try again.")
        }
        crate::heal::PatchPresentationError::Rollback { .. } if reloading_source => format!(
            "{action} failed. Disk source unchanged; reloading it and clearing edit history."
        ),
        crate::heal::PatchPresentationError::Rollback { .. } => {
            format!("{action} failed. Disk source unchanged; reopen it. Edit history was cleared.")
        }
    }
}

fn single_trash_result_message(has_receipt: bool, previous_undo_preserved: bool) -> &'static str {
    if has_receipt {
        "Moved to Trash. Undo with U."
    } else if previous_undo_preserved {
        "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action."
    } else {
        "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery."
    }
}

fn permanent_delete_success_message(path: &Path, previous_trash_undo: bool) -> String {
    let name = prefetch::privacy_safe_file_name(path).replace('"', "?");
    if previous_trash_undo {
        format!(
            "Permanently deleted \"{name}\". This cannot be undone; U still restores the previous Trash action."
        )
    } else {
        format!("Permanently deleted \"{name}\". This cannot be undone.")
    }
}

fn single_restore_failure_message(error: crate::curate::TrashRestoreError) -> String {
    match error {
        crate::curate::TrashRestoreError::DestinationOccupied =>
            "Restore blocked: The original folder already contains an item with that name. Move or rename it, then retry with U."
                .to_owned(),
        crate::curate::TrashRestoreError::AccessDenied =>
            "Restore blocked: Access was denied. Check permissions, then retry with U."
                .to_owned(),
        crate::curate::TrashRestoreError::OperationFailed =>
            "Restore failed: The operating system could not restore the file. Retry with U."
                .to_owned(),
        crate::curate::TrashRestoreError::MissingFromTrash =>
            "The exact item is no longer in the system Trash. No retry remains in viewr."
                .to_owned(),
        crate::curate::TrashRestoreError::AmbiguousReceipt =>
            "The exact Trash receipt is ambiguous. Use the system Trash; no retry remains in viewr."
                .to_owned(),
        crate::curate::TrashRestoreError::Unsupported =>
            "In-app restore is unsupported on this platform. Use the system Trash; no retry remains in viewr."
                .to_owned(),
        crate::curate::TrashRestoreError::InvalidReceipt =>
            "The exact Trash receipt is unavailable. Use the system Trash; no retry remains in viewr."
                .to_owned(),
    }
}

fn restore_result_message(
    restored: usize,
    retry_now: usize,
    resolve_then_retry: usize,
    manual_review: usize,
    terminal: usize,
    first_failure: Option<crate::curate::TrashRestoreError>,
    active_playlist: bool,
) -> String {
    let failure_total = retry_now + resolve_then_retry + manual_review + terminal;
    if failure_total == 0 {
        let suffix = if active_playlist {
            ""
        } else {
            "; reopen the source folder to refresh its view"
        };
        return format!("Restored {}{suffix}", file_count(restored));
    }
    if restored == 0 && failure_total == 1 {
        return first_failure.map_or_else(
            || "Restore failed. No retry remains in viewr.".to_owned(),
            single_restore_failure_message,
        );
    }

    let mut clauses = if restored == 0 {
        vec!["Nothing restored".to_owned()]
    } else {
        vec![format!("Restored {}", file_count(restored))]
    };
    if retry_now > 0 {
        clauses.push(format!("{} can retry with U", file_count(retry_now)));
    }
    if resolve_then_retry > 0 {
        let verb = if resolve_then_retry == 1 {
            "needs"
        } else {
            "need"
        };
        clauses.push(format!(
            "{} {verb} the blocking condition resolved, then U can retry",
            file_count(resolve_then_retry),
        ));
    }
    if manual_review > 0 {
        let verb = if manual_review == 1 {
            "requires"
        } else {
            "require"
        };
        clauses.push(format!(
            "{} {verb} system Trash review",
            file_count(manual_review),
        ));
    }
    if terminal > 0 {
        let verb = if terminal == 1 { "is" } else { "are" };
        clauses.push(format!(
            "{} {verb} no longer available for in-app restore",
            file_count(terminal),
        ));
    }
    if !active_playlist && restored > 0 {
        clauses.push("reopen the source folder to refresh its view".to_owned());
    }
    format!("{}.", clauses.join("; "))
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
    Ok(if replacing_latest {
        format!(
            "Heal source {} of {}",
            result.candidate_index + 1,
            result.candidate_count
        )
    } else {
        "Spot healed. Undo is available.".to_owned()
    })
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
    session: crate::session::Session,
    playlist: Option<Playlist>,
    playlist_scope: Option<Arc<PlaylistScope>>,
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
    is_fullscreen: bool,
    last_trashed: Vec<TrashedFile>,
    last_trashed_scope: Option<Arc<PlaylistScope>>,
    current_image: Option<Arc<DecodedImage>>,
    /// Live handle for the exact source object that supplied the displayed pixels.
    current_source: Option<Arc<crate::fs::ImageSource>>,
    /// Whether the displayed pixels are a pristine source decode safe to cache.
    current_image_reuse: ImageReuseEligibility,
    /// Timed frames for the current GIF, WebP, or APNG.
    animation: Option<crate::animated::AnimationPlayback>,
    /// Best-effort facts for the current Image Information panel.
    image_details: Option<crate::image_info::ImageDetails>,
    /// Replace-latest animation and metadata result for the current source.
    auxiliary_job: Option<OneShotJob<AuxiliaryLoadContext, AuxiliaryLoadResult>>,
    /// Generation-cancellable Windows source verification before native handoff.
    #[cfg(target_os = "windows")]
    open_with_job: Option<OneShotJob<OpenWithContext, crate::fs::ImageSourceMatch>>,
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
    /// Abnormal startup fallback to announce once the first window is ready.
    appearance_recovery: Option<PreferenceRecovery>,
    /// Whether the accessible About window is open.
    show_about: bool,
    /// Whether the accessible local update-instructions window is open.
    show_update: bool,
    /// Whether another app may have changed the source since the last accepted decode.
    external_edit_pending: bool,
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

fn rating_assignment_for_key(key: &str, repeat: bool) -> Option<RatingAssignment> {
    if repeat {
        return None;
    }
    match key {
        "0" => Some(RatingAssignment::Clear),
        "1" | "2" | "3" | "4" | "5" => {
            crate::ratings::Rating::new(key.as_bytes()[0] - b'0').map(RatingAssignment::Set)
        }
        _ => None,
    }
}

fn application_shortcuts_blocked<const N: usize>(owners: [bool; N]) -> bool {
    owners.into_iter().any(|owner| owner)
}

fn save_overwrite_dispatch_allows(
    owns_dispatch: &mut bool,
    overwrite_pending: bool,
    action: &crate::ui::UiAction,
) -> bool {
    *owns_dispatch |= overwrite_pending;
    !*owns_dispatch || crate::ui::save_overwrite_action_allowed(action)
}

fn is_space_key(key: &winit::keyboard::Key) -> bool {
    use winit::keyboard::{Key, NamedKey};

    matches!(key, Key::Named(NamedKey::Space))
        || matches!(key, Key::Character(character) if character.as_str() == " ")
}

fn space_release_must_unwind(
    key: &winit::keyboard::Key,
    state: winit::event::ElementState,
    space_held: bool,
) -> bool {
    space_held && state == winit::event::ElementState::Released && is_space_key(key)
}

const fn next_presented_rating(
    current: RatingState,
    transition: PresentedRatingTransition,
) -> RatingState {
    match transition {
        PresentedRatingTransition::Retain => current,
        PresentedRatingTransition::Replace(rating) => rating,
        PresentedRatingTransition::Clear => RatingState::Loading,
    }
}

const fn next_rating_recovery_state(current: bool, transition: RatingRecoveryTransition) -> bool {
    match transition {
        RatingRecoveryTransition::Retain => current,
        RatingRecoveryTransition::MarkUnsettled => true,
        RatingRecoveryTransition::AcceptSource => false,
    }
}

const fn rating_recovery_blocker(unsettled: bool) -> Option<&'static str> {
    if unsettled {
        Some(crate::ui::RATING_RECOVERY_STATUS)
    } else {
        None
    }
}

const fn rating_recovery_after_presentation(
    kind: PresentationKind,
    accepted_source: bool,
) -> RatingRecoveryTransition {
    if matches!(kind, PresentationKind::Loaded) && accepted_source {
        RatingRecoveryTransition::AcceptSource
    } else {
        RatingRecoveryTransition::Retain
    }
}

fn rating_discovery_transition(
    filter: RatingFilter,
    worker_active: bool,
    has_loading_ratings: bool,
) -> RatingDiscoveryTransition {
    if filter == RatingFilter::All || !has_loading_ratings {
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

fn rating_write_completion<T>(
    poll: WorkerPoll<Result<T, RatingWriteError>>,
    worker_panicked: bool,
) -> Option<Result<T, RatingWriteError>> {
    match (poll, worker_panicked) {
        (WorkerPoll::Pending, _) => None,
        (WorkerPoll::Ready(result), false) => Some(result),
        (WorkerPoll::Ready(_) | WorkerPoll::Disconnected, _) => {
            Some(Err(RatingWriteError::RecoveryFailed))
        }
    }
}

const fn exit_after_rating_write(
    close_requested: bool,
    terminal_error: Option<RatingWriteError>,
) -> bool {
    close_requested && !matches!(terminal_error, Some(RatingWriteError::RecoveryFailed))
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

fn route_consumed_keyboard_key(
    key: &winit::keyboard::Key,
    is_cropping: bool,
    is_healing: bool,
) -> bool {
    use winit::keyboard::{Key, NamedKey};

    match key {
        Key::Character(character) => {
            let character = character.as_str();
            matches!(character, "+" | "=" | "-" | "_" | "/")
                || [
                    "o", "t", "g", "i", "r", "l", "h", "v", "s", "c", "j", "u", "f", "z", "y",
                ]
                .iter()
                .any(|shortcut| character.eq_ignore_ascii_case(shortcut))
                || (is_cropping && character.eq_ignore_ascii_case("x"))
        }
        Key::Named(
            NamedKey::ArrowRight
            | NamedKey::ArrowLeft
            | NamedKey::Home
            | NamedKey::End
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::F5,
        ) => true,
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp) => is_cropping,
        Key::Named(NamedKey::Escape) => is_cropping || is_healing,
        _ => false,
    }
}

fn is_trash_shortcut_key(key: &winit::keyboard::Key) -> bool {
    use winit::keyboard::{Key, NamedKey};

    matches!(key, Key::Named(NamedKey::Delete))
        || (cfg!(target_os = "macos") && matches!(key, Key::Named(NamedKey::Backspace)))
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

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenWithOutcome {
    Launched,
    Cancelled,
    InvalidPath,
    Failed(u32),
}

#[cfg(target_os = "windows")]
fn classify_open_with_hresult(result: i32) -> OpenWithOutcome {
    const HRESULT_CANCELLED: u32 = 0x8007_04c7;
    match result {
        0 => OpenWithOutcome::Launched,
        value if value.cast_unsigned() == HRESULT_CANCELLED => OpenWithOutcome::Cancelled,
        value => OpenWithOutcome::Failed(value.cast_unsigned()),
    }
}

#[cfg(target_os = "windows")]
fn show_windows_open_with_dialog(
    path: &Path,
    parent: windows_sys::Win32::Foundation::HWND,
) -> OpenWithOutcome {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{OAIF_EXEC, OPENASINFO, SHOpenWithDialog};

    let mut path_wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if path_wide.contains(&0) {
        return OpenWithOutcome::InvalidPath;
    }
    path_wide.push(0);
    let request = OPENASINFO {
        pcszFile: path_wide.as_ptr(),
        pcszClass: std::ptr::null(),
        oaifInFlags: OAIF_EXEC,
    };
    // SAFETY: `path_wide` remains alive and NUL-terminated for the synchronous
    // call, `request` contains valid pointers, and `parent` is either viewr's
    // live HWND or null as explicitly accepted by the Windows API.
    classify_open_with_hresult(unsafe { SHOpenWithDialog(parent, &raw const request) })
}

impl App {
    fn open_file_request(&mut self, path: PathBuf) {
        self.load_and_scan(path);
    }

    fn load_and_scan(&mut self, path: PathBuf) {
        if self.block_action_while_curating("opening another image") {
            return;
        }
        let path = crate::fs::canonical_file_path(&path).unwrap_or(path);
        self.reset_prefetch_for_playlist_change();
        self.playlist = None;
        self.playlist_scope = None;
        self.begin_image_load(path.clone());
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_owned();
        self.start_folder_scan(directory, ScanPurpose::SelectedFile(path));
    }

    fn begin_image_load(&mut self, path: PathBuf) {
        #[cfg(target_os = "windows")]
        self.cancel_open_with_check();
        self.cancel_save_overwrite_for_source_change();
        self.session.selected_path = Some(path.clone());
        self.transform = Transform::default();
        self.spawn_image_load(path);
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn open_image_dialog(&mut self) {
        if self.block_action_while_curating("opening another image") {
            return;
        }
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
        if let Some(directory) = rfd::FileDialog::new().pick_folder() {
            self.start_folder_scan(directory, ScanPurpose::OpenFolder);
        }
    }

    fn start_folder_scan(&mut self, directory: PathBuf, purpose: ScanPurpose) {
        self.folder_scan_job = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let event_proxy = self.event_proxy.clone();
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
                let files = crate::fs::scan_image_entries_while(&directory, || {
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
        self.prefetch.clear();
        self.prefetch_sources.clear();
        self.prefetch_schedule.reset();
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
        self.pending_rating_write = None;
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

    fn finish_folder_scan(
        &mut self,
        purpose: ScanPurpose,
        files: Result<Vec<crate::fs::ScannedImage>, crate::fs::ScanImagesError>,
    ) -> bool {
        if let ScanPurpose::SelectedFile(selected) = &purpose
            && !selected_scan_is_current(self.session.selected_path.as_deref(), selected)
        {
            return false;
        }
        match (purpose, files) {
            (ScanPurpose::SelectedFile(selected), Ok(files)) => {
                if let Some(index) =
                    selected_file_index_by(&files, &selected, crate::fs::ScannedImage::path)
                {
                    self.replace_playlist_from_scan(files, index);
                } else {
                    self.replace_playlist(vec![selected.clone()], 0);
                }
                self.preserve_presented_source_provenance(&selected);
                self.kick_prefetch();
            }
            (
                ScanPurpose::SelectedFile(selected),
                Err(
                    crate::fs::ScanImagesError::LimitExceeded
                    | crate::fs::ScanImagesError::PathBudgetExceeded,
                ),
            ) => {
                self.replace_playlist(vec![selected.clone()], 0);
                self.preserve_presented_source_provenance(&selected);
                self.show_toast(
                    "Folder is too large for safe automatic browsing. Opened only the selected image",
                );
            }
            (
                ScanPurpose::SelectedFile(_) | ScanPurpose::OpenFolder,
                Err(crate::fs::ScanImagesError::Cancelled),
            ) => {
                return false;
            }
            (ScanPurpose::SelectedFile(selected), Err(error)) => {
                log::warn!("folder scan unavailable: {error}");
                self.replace_playlist(vec![selected.clone()], 0);
                self.preserve_presented_source_provenance(&selected);
                self.show_toast("Folder browsing is unavailable. Opened only the selected image");
            }
            (ScanPurpose::OpenFolder, Ok(files)) if files.is_empty() => {
                self.show_toast("The selected folder contains no supported images");
            }
            (ScanPurpose::OpenFolder, Ok(files)) => {
                let first = files[0].path().to_owned();
                self.replace_playlist_from_scan(files, 0);
                self.begin_image_load(first);
                self.kick_prefetch();
            }
            (
                ScanPurpose::OpenFolder,
                Err(
                    crate::fs::ScanImagesError::LimitExceeded
                    | crate::fs::ScanImagesError::PathBudgetExceeded,
                ),
            ) => {
                self.show_toast("The selected folder exceeds safe browsing limits");
            }
            (ScanPurpose::OpenFolder, Err(error)) => {
                log::warn!("selected folder scan failed: {error}");
                self.show_toast("Could not read the selected folder");
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
        let recovery_transition =
            rating_recovery_after_presentation(kind, self.current_source.is_some());
        self.rating_recovery_unsettled =
            next_rating_recovery_state(self.rating_recovery_unsettled, recovery_transition);
        self.external_edit_pending = external_edit_pending_after_frame_transition(
            self.external_edit_pending,
            PresentedFrameTransition::Present(kind),
        );
        match kind {
            PresentationKind::Loaded => {
                self.prefetch_schedule.allow(path);
                self.start_auxiliary_load(path);
            }
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
        self.external_edit_pending = external_edit_pending_after_frame_transition(
            self.external_edit_pending,
            PresentedFrameTransition::Invalidate,
        );
        self.heal.reset_for_image();
        self.cancel_crop_work();
        self.preview_job = None;
        self.preview_load_retry_blocked = false;
        self.animation = None;
        self.image_details = None;
        self.auxiliary_job = None;
        self.session.load_error = None;
        self.current_image = None;
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
    fn prepare_for_image_load(&mut self) {
        self.external_edit_pending = external_edit_pending_after_frame_transition(
            self.external_edit_pending,
            PresentedFrameTransition::RetainForReplacement,
        );
        self.heal.reset_for_image();
        self.cancel_crop_work();
        self.preview_job = None;
        self.preview_load_retry_blocked = false;
        self.animation = None;
        self.auxiliary_job = None;
        self.current_rating_capability = RatingWriteCapability::UnsafeSource;
        self.presented_rating =
            next_presented_rating(self.presented_rating, PresentedRatingTransition::Retain);
        self.rating_recovery_unsettled = next_rating_recovery_state(
            self.rating_recovery_unsettled,
            RatingRecoveryTransition::Retain,
        );
        self.session.prepare_for_load();
    }

    fn start_auxiliary_load(&mut self, path: &Path) {
        self.animation = None;
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
            let animation = source.as_ref().map_or(Ok(None), |source| {
                crate::animated::DecodedAnimation::load_background_if_current(
                    &job_path,
                    source,
                    &current_generation,
                    generation,
                )
            });
            let animation = animation.map_err(|error| error.to_string());
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
            let _ = completion.complete((animation, details, rating));
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
        if !auxiliary_job_is_current(
            &context,
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
                self.current_image_reuse = ImageReuseEligibility::Ineligible;
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
        self.current_image_reuse = ImageReuseEligibility::Ineligible;
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
        renderer.window().request_redraw();
        Ok(())
    }

    fn pause_animation(&mut self) {
        if let Some(playback) = self.animation.as_mut() {
            playback.pause();
        }
    }

    fn request_rating_assignment(&mut self, assignment: RatingAssignment) {
        if self.block_action_while_busy("changing the rating", true) {
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
            self.start_rating_write(&pending);
        } else {
            self.pending_rating_write = Some(pending);
            self.request_redraw();
        }
    }

    fn confirm_rating_disclosure(&mut self) {
        let Some(pending) = self.pending_rating_write.take() else {
            return;
        };
        self.rating_write_disclosed = true;
        self.start_rating_write(&pending);
    }

    fn cancel_rating_disclosure(&mut self) {
        self.pending_rating_write = None;
        self.request_redraw();
    }

    fn start_rating_write(&mut self, pending: &PendingRatingWrite) {
        if self.save_transaction_active() {
            self.show_toast("Wait for Save As to finish before changing the rating");
            return;
        }
        if self.rating_write_worker.is_some()
            || self.session.presented_path.as_ref() != Some(&pending.path)
        {
            self.show_toast("The selected image changed before the rating could be saved");
            return;
        }
        let Some(source) = self.current_source.clone() else {
            self.show_toast("Wait for the selected image to finish loading");
            return;
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
        match spawned {
            Ok(join) => {
                self.rating_write_worker = Some(RatingWriteWorker {
                    path,
                    assignment,
                    result_rx: receiver,
                    join,
                });
                self.show_toast("Saving rating...");
            }
            Err(_) => self
                .show_toast("Could not save the rating safely. The previous rating is unchanged."),
        }
    }

    fn poll_rating_write(&mut self, event_loop: &ActiveEventLoop) {
        let Some(worker) = self.rating_write_worker.as_ref() else {
            return;
        };
        let poll = poll_worker(&worker.result_rx);
        if matches!(poll, WorkerPoll::Pending) {
            return;
        }
        let worker = self
            .rating_write_worker
            .take()
            .expect("rating worker exists after reaching terminal channel state");
        let worker_panicked = worker.join.join().is_err();
        if worker_panicked {
            log::error!("rating write worker panicked after terminal channel state");
        }
        let result = rating_write_completion(poll, worker_panicked)
            .expect("terminal rating channel state produces a completion");
        let terminal_error = result.as_ref().err().copied();
        let should_exit = exit_after_rating_write(
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
        if should_exit {
            event_loop.exit();
        }
    }

    fn set_rating_filter(&mut self, filter: RatingFilter) {
        if self.block_action_while_busy("changing the rating filter", true) {
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
                let mut ratings = Vec::with_capacity(files.len());
                for (path, provenance) in files {
                    if worker_cancel.load(Ordering::Acquire) {
                        return;
                    }
                    let rating = crate::ratings::scan_path_rating_while(&path, provenance, || {
                        !worker_cancel.load(Ordering::Acquire)
                    });
                    ratings.push((path, rating));
                }
                if !worker_cancel.load(Ordering::Acquire) {
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
        self.spawn_image_load(path);
        self.request_redraw();
    }

    fn reload_current_image(&mut self) {
        if self.block_action_while_curating("reloading this file") {
            return;
        }
        if self.heal.is_busy() || self.heal.painting {
            self.show_toast("Wait for spot heal to finish before reloading");
            return;
        }
        if self.crop_job.is_some() {
            self.show_toast("Wait for the crop to finish before reloading");
            return;
        }
        if self.save_transaction_active() {
            self.show_toast("Wait for Save As to finish before reloading");
            return;
        }
        if self.session.is_loading() || self.preview_job.is_some() {
            self.show_toast("An image is already loading");
            return;
        }
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        #[cfg(target_os = "windows")]
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

    #[cfg(target_os = "windows")]
    fn open_current_with(&mut self) {
        if self.block_action_while_busy("opening the source in another app", true) {
            return;
        }
        if self.open_with_job.is_some() {
            self.show_toast("Source verification for Open With is already running");
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

    #[cfg(target_os = "windows")]
    fn cancel_open_with_check(&mut self) {
        if let Some(job) = self.open_with_job.take() {
            job.context().cancel.store(true, Ordering::Release);
        }
    }

    #[cfg(target_os = "windows")]
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
        if self.session.generation.load(Ordering::Acquire) != context.generation
            || self.current_loaded_path() != Some(context.path.as_path())
        {
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

    #[cfg(target_os = "windows")]
    fn show_open_with_dialog(&mut self, path: &Path) {
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
        self.context_menu_pos = None;
        match show_windows_open_with_dialog(path, parent) {
            OpenWithOutcome::Launched => {
                self.external_edit_pending = true;
                self.show_toast(
                    "Source opened in another app. Press F5 to reload possible changes",
                );
            }
            OpenWithOutcome::Cancelled => self.show_toast("Open With canceled"),
            OpenWithOutcome::InvalidPath => {
                log::error!("Windows Open With rejected an invalid path");
                self.show_toast("Could not open the Windows app chooser");
            }
            OpenWithOutcome::Failed(code) => {
                log::error!("Windows Open With failed with HRESULT {code:#010x}");
                self.show_toast("Could not open the Windows app chooser");
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn open_current_with(&mut self) {
        self.show_toast("Open With is currently available on Windows");
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
        if self.block_action_while_busy("rotating the image", true) {
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
        if self.block_action_while_busy("flipping the image", true) {
            return;
        }
        if self.current_loaded_path().is_some() {
            self.transform.flip_h = !self.transform.flip_h;
            self.request_redraw();
        }
    }

    fn flip_current_vertically(&mut self) {
        if self.block_action_while_busy("flipping the image", true) {
            return;
        }
        if self.current_loaded_path().is_some() {
            self.transform.flip_v = !self.transform.flip_v;
            self.request_redraw();
        }
    }

    fn handle_single_key_shortcut(&mut self, key: &str) {
        if let Some(assignment) = rating_assignment_for_key(key, false) {
            self.request_rating_assignment(assignment);
            return;
        }
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
                    .is_some_and(|playlist| filmstrip_is_available(playlist.visible_len())) =>
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
            "f" | "F" => self.toggle_fullscreen(),
            "+" | "=" => self.zoom_at_viewport_center(1.15),
            "-" | "_" => self.zoom_at_viewport_center(1.0 / 1.15),
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

    fn go_to_index(&mut self, new_index: usize) {
        #[cfg(target_os = "windows")]
        self.cancel_open_with_check();
        if self.block_action_while_busy("browsing to another image", true) {
            return;
        }
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
        self.spawn_image_load_with_cached(next_path, cached_target, false);
        self.kick_prefetch();
    }

    /// Decode nearby playlist entries into the in-memory cache (no disk writes).
    fn kick_prefetch(&mut self) {
        let Some(playlist) = &self.playlist else {
            return;
        };
        let targets: Vec<(PathBuf, Option<crate::fs::ScanProvenance>)> = playlist
            .visible_neighbor_paths(2)
            .into_iter()
            .filter(|p| !self.prefetch.contains(p) && self.prefetch_schedule.is_eligible(p))
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
            let destination = prefetch_destination(
                self.session.selected_path.as_deref(),
                self.session.is_loading() || self.session.load_error.is_some(),
                self.playlist.as_ref(),
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
                        let retained = self.insert_prefetched_image(path.clone(), image);
                        if !retained {
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
        if presented && let Some(renderer) = self.renderer.as_ref() {
            renderer.window().request_redraw();
        }
    }

    fn trash_current(&mut self) {
        let Some(path) = self.current_loaded_path().map(Path::to_owned) else {
            return;
        };
        if self.block_action_while_busy("moving this file to Trash", true) {
            return;
        }
        if let Some(message) = self.curation_recovery.source_removal_preflight() {
            self.show_toast(message);
            return;
        }

        let Some(source) = self.current_source.as_ref().map(Arc::clone) else {
            let error = GuardedActionError::Unavailable;
            log_guarded_action_failure(GuardedActionKind::Trash, &error);
            self.show_toast(guarded_action_failure_message(
                GuardedActionKind::Trash,
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
        let started = self.start_curation_worker(
            "viewr-trash-move",
            context,
            move || CurationCompletion::Trash {
                result: crate::curate::move_source_to_trash(&path, &source),
            },
            "Could not start the move to Trash. Nothing was moved.",
        );
        if started {
            self.show_toast("Moving file to Trash in the background");
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

    fn block_action_while_busy(&mut self, action: &str, include_spot_heal: bool) -> bool {
        #[cfg(target_os = "windows")]
        let source_verification = self
            .open_with_job
            .is_some()
            .then_some(CurrentWork::SourceVerification);
        #[cfg(not(target_os = "windows"))]
        let source_verification = None;
        let blocker = current_work_blocker([
            self.curation_worker
                .as_ref()
                .map(|worker| worker.context.kind().work()),
            source_verification,
            self.folder_scan_job
                .is_some()
                .then_some(CurrentWork::FolderScan),
            image_preparation_work(self.session.is_loading(), self.preview_job.is_some()),
            crop_work(self.transform.is_cropping, self.crop_job.is_some()),
            self.save_transaction_active().then_some(CurrentWork::Save),
            self.rating_write_worker
                .is_some()
                .then_some(CurrentWork::RatingWrite),
            (include_spot_heal && (self.heal.active || self.heal.is_busy() || self.heal.painting))
                .then_some(CurrentWork::SpotHeal),
        ]);
        if let Some(blocker) = blocker {
            self.show_toast(blocked_action_message(action, blocker));
            true
        } else {
            false
        }
    }

    fn block_action_while_curating(&mut self, action: &str) -> bool {
        let Some(worker) = self.curation_worker.as_ref() else {
            return false;
        };
        let blocker = worker.context.kind().work();
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
            recovery,
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
        self.auxiliary_job = restored.auxiliary_job;
        self.request_redraw();
        true
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
        if self.block_action_while_curating("changing Crop") {
            return;
        }
        if let Some(message) =
            crop_source_blocker(self.session.is_loading(), self.session.load_error.is_some())
        {
            self.show_toast(message);
            return;
        }
        if self.preview_job.is_some() {
            self.show_toast("Wait for the image preview to finish before cropping");
            return;
        }
        if self.current_loaded_path().is_none() {
            return;
        }
        if self.crop_job.is_some() {
            self.show_toast("A crop is already being applied");
            return;
        }
        if self.save_transaction_active() {
            self.show_toast("Wait for the current save to finish before cropping");
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
        if self.block_action_while_curating("changing Spot Heal") {
            return;
        }
        if !self.heal.active && self.current_loaded_path().is_none() {
            return;
        }
        if !self.heal.active && self.crop_job.is_some() {
            self.show_toast("Wait for the crop to finish before using Spot Heal");
            return;
        }
        if !self.heal.active && self.preview_job.is_some() {
            self.show_toast("Wait for the image preview to finish before using Spot Heal");
            return;
        }
        if !self.heal.active && self.save_transaction_active() {
            self.show_toast("Wait for the current save to finish before using Spot Heal");
            return;
        }
        if !self.heal.active && self.rating_write_worker.is_some() {
            self.show_toast("Wait for the rating update to finish before using Spot Heal");
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
            || self.crop_job.is_some()
            || self.save_transaction_active()
            || self.preview_job.is_some()
        {
            return;
        }
        let Some(refresh) = self.heal.refresh.as_ref() else {
            return;
        };
        if refresh.candidate_count < 2 {
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
        if !self.heal.active
            || self.heal.is_busy()
            || self.crop_job.is_some()
            || self.save_transaction_active()
            || self.preview_job.is_some()
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
        if self.block_action_while_busy("undoing an edit", true) {
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
        if self.block_action_while_busy("redoing an edit", true) {
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
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |s| s.to_string_lossy().into_owned(),
                );
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
                        let id = format!("thumb:{}", path.display());
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
        if self.block_action_while_busy("permanently deleting this file", true) {
            return;
        }
        if let Some(message) = self.curation_recovery.source_removal_preflight() {
            self.show_toast(message);
            return;
        }
        let Some(source) = self.current_source.as_ref().map(Arc::clone) else {
            let error = GuardedActionError::Unavailable;
            log_guarded_action_failure(GuardedActionKind::PermanentDelete, &error);
            self.show_toast(guarded_action_failure_message(
                GuardedActionKind::PermanentDelete,
                &error,
            ));
            return;
        };
        if let Err(error) = crate::curate::verify_accepted_source_native(&path, &source) {
            log_guarded_action_failure(GuardedActionKind::PermanentDelete, &error);
            self.show_toast(guarded_action_failure_message(
                GuardedActionKind::PermanentDelete,
                &error,
            ));
            return;
        }
        let confirmed = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Permanently delete?")
            .set_description(permanent_delete_description(&path))
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                PERMANENT_DELETE_ACTION.to_owned(),
                "Cancel".to_owned(),
            ))
            .show();
        if !permanent_delete_confirmed(&confirmed) {
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
                log_guarded_action_failure(GuardedActionKind::Trash, &error);
                self.show_toast(guarded_action_failure_message(
                    GuardedActionKind::Trash,
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
            log_guarded_action_failure(GuardedActionKind::PermanentDelete, &error);
            self.show_toast(guarded_action_failure_message(
                GuardedActionKind::PermanentDelete,
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
        self.show_toast(permanent_delete_success_message(
            &context.path,
            previous_trash_undo,
        ));
        CurationTerminalState::Succeeded
    }

    fn after_paths_removed(&mut self, removed: &[PathBuf], old_index: usize) {
        self.reset_prefetch_for_playlist_change();
        if let Some(playlist) = &mut self.playlist {
            playlist.remove_paths(removed, old_index);
            if playlist.files.is_empty() {
                self.cancel_pending_image_load();
                self.session.selected_path = None;
                self.invalidate_displayed_image();
            } else {
                if playlist.visible_len() == 0 {
                    self.cancel_pending_image_load();
                    self.session.selected_path = None;
                    self.invalidate_displayed_image();
                    return;
                }
                let next_path = playlist.files[playlist.index].clone();
                self.session.selected_path = Some(next_path.clone());
                self.transform = Transform::default();
                self.spawn_image_load(next_path);
            }
        } else {
            self.cancel_pending_image_load();
            self.session.selected_path = None;
            self.invalidate_displayed_image();
        }
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
        if self.block_action_while_busy("restoring files from Trash", true) {
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
        let cached_image = self.take_prefetched_image(&path);
        self.spawn_image_load_with_cached(path, cached_image, false);
    }

    fn spawn_refreshed_image_load(&mut self, path: PathBuf) {
        self.spawn_image_load_with_cached(path, None, true);
    }

    fn spawn_image_load_with_cached(
        &mut self,
        path: PathBuf,
        cached_image: Option<LoadedImage>,
        refresh_scanned: bool,
    ) {
        let generation = self
            .session
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.prepare_for_image_load();

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
                Err(error) => Err(error.to_string()),
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
        let terminal = match polled {
            JobPoll::Ready(Ok(crate::edit::MetadataDisposition::Retained)) => {
                self.show_toast("Saved copy · EXIF retained");
                SaveTerminalState::Succeeded
            }
            JobPoll::Ready(Ok(crate::edit::MetadataDisposition::NotPresent)) => {
                self.show_toast("Saved copy · no EXIF found");
                SaveTerminalState::Succeeded
            }
            JobPoll::Ready(Ok(crate::edit::MetadataDisposition::Stripped)) => {
                self.show_toast("Saved copy · metadata stripped");
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
        if self.block_action_while_curating("applying the crop") {
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
        if self.crop_job.is_some() {
            self.show_toast("A crop is already being applied");
            return;
        }
        if self.preview_job.is_some() {
            self.show_toast("Wait for the image preview to finish before cropping");
            return;
        }
        if self.save_transaction_active() {
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
        let Some(pixel_rect) = crate::crop::crop_pixel_rect(rect, image.width, image.height, ratio)
        else {
            self.show_toast("The selected ratio is too large for this image");
            return;
        };

        let source_generation = self.session.generation.load(Ordering::Acquire);
        let source_image = Arc::clone(&image);
        let source_transform = self.transform;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let recovery = CropRecovery {
            source_path,
            source_generation,
            source_image,
            transform: source_transform,
            animation: self.animation.take(),
            auxiliary_job: self.auxiliary_job.take(),
        };
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
                if let Some(recovery) = self.appearance_recovery.take() {
                    self.show_toast(recovery.notice());
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
            let response = renderer.egui_state.on_window_event(window.as_ref(), &event);
            // egui reports that RedrawRequested itself wants repainting. The
            // current event already satisfies that request, so scheduling it
            // again here would create a permanent redraw loop.
            if response.repaint && !is_redraw_event {
                egui_requested_repaint = true;
                window.request_redraw();
            }
            egui_consumed = response.consumed;
            egui_popup_open = egui::Popup::is_any_open(&renderer.egui_ctx);
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
                    space_release_must_unwind(&event.logical_key, event.state, self.space_held)
                        || (!application_shortcuts_blocked([
                            self.show_about,
                            self.show_update,
                            self.pending_save.is_some(),
                            self.pending_rating_write.is_some(),
                            egui_popup_open,
                            self.context_menu_pos.is_some(),
                        ]) && route_consumed_keyboard_key(
                            &event.logical_key,
                            self.transform.is_cropping,
                            self.heal.active,
                        ))
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
                self.open_file_request(path);
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
                if is_space && !pressed {
                    if self.space_held {
                        self.space_held = false;
                        self.update_cursor_icon();
                        if !self.space_dragged {
                            self.transform = Transform::default();
                            if let Some(renderer) = self.renderer.as_mut() {
                                renderer.window().request_redraw();
                            }
                        }
                        self.space_dragged = false;
                    }
                    return;
                }
                if application_shortcuts_blocked([
                    self.show_about,
                    self.show_update,
                    self.pending_save.is_some(),
                    self.pending_rating_write.is_some(),
                    egui_popup_open,
                    self.context_menu_pos.is_some(),
                ]) {
                    return;
                }
                // Space: hold = temporary hand tool; tap (no drag) = reset view.
                if is_space {
                    if self.heal.painting {
                        self.finish_heal_stroke();
                    }
                    self.space_held = true;
                    self.space_dragged = false;
                    self.update_cursor_icon();
                    return;
                }
                if !pressed {
                    return;
                }
                if matches!(&logical_key, Key::Character(value) if repeat && rating_assignment_for_key(value.as_str(), false).is_some())
                {
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
                        current_catalog_index: Some(playlist.index),
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
                let path_str = self
                    .session
                    .presented_path
                    .as_ref()
                    .or(self.session.selected_path.as_ref())
                    .map(|p| p.to_string_lossy().into_owned());
                let selected_file_name = self
                    .session
                    .selected_path
                    .as_deref()
                    .map(prefetch::privacy_safe_file_name);
                let retain_exif = self.retain_exif;
                let theme_preference = self.theme_preference;
                let show_about = self.show_about;
                let show_update = self.show_update;
                let external_edit_pending = self.external_edit_pending;
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
                    rating,
                    external_edit_pending,
                    file_path: path_str,
                    selected_file_name,
                    img_size,
                    animation,
                    details,
                    color_profile,
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
                    restore_recovery_unsettled,
                    is_panning,
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

                let mut save_overwrite_owns_dispatch = false;
                for action in ui_actions {
                    if !save_overwrite_dispatch_allows(
                        &mut save_overwrite_owns_dispatch,
                        self.pending_save.is_some(),
                        &action,
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
                        crate::ui::UiAction::ShowAbout => {
                            self.show_about = true;
                            self.show_update = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::CloseAbout => {
                            self.show_about = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::ShowUpdate => {
                            self.show_update = true;
                            self.show_about = false;
                            self.request_redraw();
                        }
                        crate::ui::UiAction::CloseUpdate => {
                            self.show_update = false;
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
        #[cfg(target_os = "windows")]
        self.finish_open_with_check();
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
        if let Some(rx) = &self.session.receiver
            && let Ok((path, result)) = rx.try_recv()
        {
            self.session.receiver = None;
            let is_current = self.session.selected_path.as_ref() == Some(&path);
            match result {
                Ok(image) if is_current => {
                    self.display_loaded_image(&path, image);
                    self.kick_prefetch();
                    if let Some(r) = self.renderer.as_mut() {
                        r.window().request_redraw();
                    }
                }
                Err(e) if is_current => {
                    log::error!("decode failed");
                    let message = format!("Could not decode: {e}");
                    self.session.load_error = Some(message.clone());
                    self.show_toast(format!(
                        "{message}. The previous image remains visible; Retry is available."
                    ));
                    if let Some(r) = self.renderer.as_mut() {
                        r.window().request_redraw();
                    }
                }
                Ok(_) | Err(_) => {}
            }
        }

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrefetchDestination {
    PresentSelected,
    CacheNeighbor,
    Ignore,
}

fn prefetch_destination(
    selected: Option<&Path>,
    selected_is_pending_or_failed: bool,
    playlist: Option<&Playlist>,
    path: &Path,
) -> PrefetchDestination {
    if selected == Some(path) {
        if selected_is_pending_or_failed {
            PrefetchDestination::PresentSelected
        } else {
            PrefetchDestination::Ignore
        }
    } else if playlist.is_some_and(|playlist| playlist.files.iter().any(|item| item == path)) {
        PrefetchDestination::CacheNeighbor
    } else {
        PrefetchDestination::Ignore
    }
}

fn selected_file_index_by<T>(
    files: &[T],
    selected: &Path,
    path: impl Fn(&T) -> &Path + Copy,
) -> Option<usize> {
    files
        .iter()
        .position(|entry| path(entry) == selected)
        .or_else(|| {
            let selected_name = selected.file_name()?;
            files
                .iter()
                .position(|entry| path(entry).file_name() == Some(selected_name))
        })
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

fn selected_scan_is_current(current: Option<&Path>, selected: &Path) -> bool {
    current == Some(selected)
}

fn auxiliary_job_is_current(
    context: &AuxiliaryLoadContext,
    generation: u64,
    selected: Option<&Path>,
    presented: Option<&Path>,
) -> bool {
    context.generation == generation
        && selected == Some(context.path.as_path())
        && presented == Some(context.path.as_path())
}

const fn auxiliary_disconnect_message() -> &'static str {
    "Image details, animation, and rating reading stopped unexpectedly. Close and reopen viewr before continuing."
}

const fn rating_after_auxiliary_disconnect() -> RatingObservation {
    RatingObservation {
        state: RatingState::Unreadable,
        capability: RatingWriteCapability::ObservationFailed,
    }
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

fn crop_recovery_matches(
    recovery: &CropRecovery,
    current_generation: u64,
    selected_path: Option<&Path>,
    presented_path: Option<&Path>,
    current_image: Option<&Arc<DecodedImage>>,
) -> bool {
    recovery.source_generation == current_generation
        && selected_path == Some(recovery.source_path.as_path())
        && presented_path == Some(recovery.source_path.as_path())
        && current_image.is_some_and(|image| Arc::ptr_eq(image, &recovery.source_image))
}

const fn crop_failure_message(selection_restored: bool) -> &'static str {
    if selection_restored {
        "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again."
    } else {
        "Crop was not applied because the image changed."
    }
}

const fn crop_disconnect_message(selection_restored: bool) -> &'static str {
    if selection_restored {
        "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
    } else {
        "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
    }
}

const fn crop_preview_disconnect_message(selection_restored: bool) -> &'static str {
    if selection_restored {
        "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
    } else {
        "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
    }
}

const fn crop_recovery_blocker(
    crop_recovery_unsettled: bool,
    preview_recovery_unsettled: bool,
) -> Option<&'static str> {
    if crop_recovery_unsettled {
        Some(crate::ui::CROP_RECOVERY_STATUS)
    } else if preview_recovery_unsettled {
        Some(crate::ui::PREVIEW_RECOVERY_STATUS)
    } else {
        None
    }
}

const fn preview_retry_blocker(preview_load_retry_blocked: bool) -> Option<&'static str> {
    if preview_load_retry_blocked {
        Some(crate::ui::PREVIEW_RECOVERY_STATUS)
    } else {
        None
    }
}

const fn crop_source_blocker(
    image_open_in_progress: bool,
    image_open_failed: bool,
) -> Option<&'static str> {
    if image_open_in_progress {
        Some("Wait for the image to finish opening before cropping")
    } else if image_open_failed {
        Some("Retry the failed image load before cropping")
    } else {
        None
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

fn permanent_delete_description(path: &Path) -> String {
    let name = prefetch::privacy_safe_file_name(path).replace('"', "?");
    format!(
        "Delete \"{name}\" forever?\n\nThis skips the system Trash and cannot be undone from viewr."
    )
}

fn permanent_delete_confirmed(result: &rfd::MessageDialogResult) -> bool {
    matches!(
        result,
        rfd::MessageDialogResult::Custom(label) if label == PERMANENT_DELETE_ACTION
    )
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
    fn only_an_explicit_open_folder_scan_blocks_save_preflight() {
        let selected = ScanPurpose::SelectedFile(PathBuf::from("selected.png"));

        assert!(folder_scan_blocks_save(Some(&ScanPurpose::OpenFolder)));
        assert!(!folder_scan_blocks_save(Some(&selected)));
        assert!(!folder_scan_blocks_save(None));
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

        assert!(save_overwrite_dispatch_allows(
            &mut owns_dispatch,
            false,
            &crate::ui::UiAction::SaveAs,
        ));
        assert!(!save_overwrite_dispatch_allows(
            &mut owns_dispatch,
            true,
            &crate::ui::UiAction::Trash,
        ));
        assert!(save_overwrite_dispatch_allows(
            &mut owns_dispatch,
            true,
            &crate::ui::UiAction::CancelSaveOverwrite,
        ));
        assert!(!save_overwrite_dispatch_allows(
            &mut owns_dispatch,
            false,
            &crate::ui::UiAction::Open,
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
            ));
        }
    }

    #[test]
    fn menus_modals_and_popups_own_keyboard_shortcuts() {
        assert!(!application_shortcuts_blocked([false; 6]));
        for owner in 0..6 {
            let mut owners = [false; 6];
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
    fn failed_replacement_load_retains_the_presented_rating() {
        let rating = RatingState::Rated(crate::ratings::Rating::new(4).unwrap());

        assert_eq!(
            next_presented_rating(rating, PresentedRatingTransition::Retain),
            rating
        );
        assert_eq!(
            next_presented_rating(rating, PresentedRatingTransition::Clear),
            RatingState::Loading
        );
        assert_eq!(
            next_presented_rating(
                rating,
                PresentedRatingTransition::Replace(RatingState::Unrated)
            ),
            RatingState::Unrated
        );
    }

    #[test]
    fn threshold_changes_reuse_one_active_rating_scan() {
        let threshold = RatingFilter::AtLeast(crate::ratings::Rating::new(4).unwrap());

        assert_eq!(
            rating_discovery_transition(threshold, true, true),
            RatingDiscoveryTransition::KeepRunning
        );
        assert_eq!(
            rating_discovery_transition(threshold, false, true),
            RatingDiscoveryTransition::Start
        );
        assert_eq!(
            rating_discovery_transition(RatingFilter::All, true, true),
            RatingDiscoveryTransition::CancelAndApply
        );
        assert_eq!(
            rating_discovery_transition(threshold, true, false),
            RatingDiscoveryTransition::CancelAndApply
        );
    }

    #[test]
    fn rating_worker_loss_is_indeterminate_and_blocks_deferred_exit() {
        assert_eq!(
            rating_write_completion(WorkerPoll::Ready(Ok(7_u8)), false),
            Some(Ok(7))
        );
        assert_eq!(
            rating_write_completion(WorkerPoll::Ready(Ok(7_u8)), true),
            Some(Err(RatingWriteError::RecoveryFailed))
        );
        assert_eq!(
            rating_write_completion::<u8>(WorkerPoll::Disconnected, false),
            Some(Err(RatingWriteError::RecoveryFailed))
        );
        assert!(!exit_after_rating_write(
            true,
            Some(RatingWriteError::RecoveryFailed)
        ));
        assert!(exit_after_rating_write(
            true,
            Some(RatingWriteError::WriteFailed)
        ));
    }

    #[test]
    fn rating_recovery_clears_only_after_an_accepted_source() {
        let unsettled = next_rating_recovery_state(false, RatingRecoveryTransition::MarkUnsettled);
        assert!(unsettled);
        assert!(next_rating_recovery_state(
            unsettled,
            RatingRecoveryTransition::Retain
        ));
        assert!(!next_rating_recovery_state(
            unsettled,
            RatingRecoveryTransition::AcceptSource
        ));
        assert_eq!(
            rating_recovery_blocker(unsettled),
            Some(crate::ui::RATING_RECOVERY_STATUS)
        );
        assert_eq!(rating_recovery_blocker(false), None);
        assert_eq!(
            rating_recovery_after_presentation(PresentationKind::Cropped, true),
            RatingRecoveryTransition::Retain
        );
        assert_eq!(
            rating_recovery_after_presentation(PresentationKind::Loaded, false),
            RatingRecoveryTransition::Retain
        );
        assert_eq!(
            rating_recovery_after_presentation(PresentationKind::Loaded, true),
            RatingRecoveryTransition::AcceptSource
        );
    }

    #[test]
    fn auxiliary_result_requires_exact_generation_and_both_image_owners() {
        let path = PathBuf::from("current.jpg");
        let other = Path::new("other.jpg");
        let context = AuxiliaryLoadContext {
            path: path.clone(),
            generation: 8,
        };

        assert!(auxiliary_job_is_current(
            &context,
            8,
            Some(&path),
            Some(&path)
        ));
        assert!(!auxiliary_job_is_current(
            &context,
            7,
            Some(&path),
            Some(&path)
        ));
        assert!(!auxiliary_job_is_current(
            &context,
            8,
            Some(other),
            Some(&path)
        ));
        assert!(!auxiliary_job_is_current(
            &context,
            8,
            Some(&path),
            Some(other)
        ));
    }

    #[test]
    fn auxiliary_disconnect_copy_requires_a_restart_without_promising_success() {
        let message = auxiliary_disconnect_message();

        assert!(message.contains("Close and reopen viewr"));
        assert!(message.contains("rating"));
        assert!(!message.contains("recover"));
        assert!(!message.contains("F5"));
        assert!(!message.contains(['\\', '/']));
        assert_eq!(
            rating_after_auxiliary_disconnect(),
            RatingObservation {
                state: RatingState::Unreadable,
                capability: RatingWriteCapability::ObservationFailed,
            }
        );
    }

    #[test]
    fn filmstrip_layout_uses_the_projected_entry_count() {
        assert!(!filmstrip_is_available(0));
        assert!(!filmstrip_is_available(1));
        assert!(filmstrip_is_available(2));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_open_with_hresult_is_classified_without_path_details() {
        assert_eq!(classify_open_with_hresult(0), OpenWithOutcome::Launched);
        assert_eq!(
            classify_open_with_hresult(0x8007_04c7_u32.cast_signed()),
            OpenWithOutcome::Cancelled
        );
        assert_eq!(
            classify_open_with_hresult(0x8000_4005_u32.cast_signed()),
            OpenWithOutcome::Failed(0x8000_4005)
        );
    }

    #[test]
    fn busy_action_copy_is_specific_and_prioritized() {
        assert_eq!(crop_work(true, false), Some(CurrentWork::Crop));
        assert_eq!(crop_work(false, true), Some(CurrentWork::Crop));
        assert_eq!(crop_work(false, false), None);
        assert_eq!(
            current_work_blocker([
                None,
                None,
                image_preparation_work(true, false),
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::ImagePreparation)
        );
        assert_eq!(
            current_work_blocker([
                None,
                None,
                image_preparation_work(false, true),
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::ImagePreparation)
        );
        assert_eq!(
            current_work_blocker([
                None,
                None,
                None,
                Some(CurrentWork::Crop),
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::Crop)
        );
        assert_eq!(
            current_work_blocker([
                None,
                None,
                None,
                None,
                Some(CurrentWork::Save),
                Some(CurrentWork::SpotHeal),
            ]),
            Some(CurrentWork::Save)
        );
        assert_eq!(
            current_work_blocker([None, None, None, None, None, Some(CurrentWork::SpotHeal)]),
            Some(CurrentWork::SpotHeal)
        );
        assert_eq!(
            current_work_blocker([
                Some(CurrentWork::TrashRestore),
                Some(CurrentWork::FolderScan),
                None,
                None,
                None,
                None,
            ]),
            Some(CurrentWork::TrashRestore)
        );
        assert_eq!(
            current_work_blocker([None, Some(CurrentWork::FolderScan), None, None, None, None,]),
            Some(CurrentWork::FolderScan)
        );
        assert_eq!(
            current_work_blocker([None, None, None, None, None, None]),
            None
        );
        assert_eq!(
            blocked_action_message("moving this file to Trash", CurrentWork::SpotHeal),
            "Wait for Spot Heal to finish before moving this file to Trash"
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            blocked_action_message("saving a copy", CurrentWork::SourceVerification),
            "Wait for source verification to finish before saving a copy"
        );
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
    fn close_and_curation_preflight_follow_app_ownership() {
        assert_eq!(close_disposition(false, false), CloseDisposition::Exit);
        assert_eq!(
            close_disposition(true, false),
            CloseDisposition::WaitForSave
        );
        assert_eq!(
            close_disposition(false, true),
            CloseDisposition::WaitForCuration
        );
        assert_eq!(
            close_disposition(true, true),
            CloseDisposition::WaitForSaveAndCuration
        );
        assert_eq!(
            curation_action_preflight(
                Some(CurationKind::Restore),
                false,
                "restoring files from Trash",
                "Nothing to restore from Trash",
            ),
            Some(
                "Wait for the Trash restore to finish before restoring files from Trash".to_owned()
            )
        );
        assert_eq!(
            curation_action_preflight(
                None,
                false,
                "restoring files from Trash",
                "Nothing to restore from Trash",
            ),
            Some("Nothing to restore from Trash".to_owned())
        );
    }

    #[test]
    fn save_start_preflight_excludes_source_changes_writes_and_unsettled_recovery() {
        use SaveStartBlocker::{Crop, FolderOpen, Preview, RatingWrite, Recovery, Save, SpotHeal};

        let cases = [
            (
                [
                    Some(Recovery),
                    Some(FolderOpen),
                    Some(RatingWrite),
                    Some(Preview),
                    Some(SpotHeal),
                    Some(Crop),
                    Some(Save),
                ],
                Some(Recovery),
            ),
            (
                [None, Some(FolderOpen), None, None, None, None, None],
                Some(FolderOpen),
            ),
            (
                [None, None, Some(RatingWrite), None, None, None, None],
                Some(RatingWrite),
            ),
            (
                [
                    None,
                    None,
                    None,
                    Some(Preview),
                    Some(SpotHeal),
                    Some(Crop),
                    Some(Save),
                ],
                Some(Preview),
            ),
            (
                [
                    None,
                    None,
                    None,
                    None,
                    Some(SpotHeal),
                    Some(Crop),
                    Some(Save),
                ],
                Some(SpotHeal),
            ),
            (
                [None, None, None, None, None, Some(Crop), Some(Save)],
                Some(Crop),
            ),
            ([None, None, None, None, None, None, Some(Save)], Some(Save)),
            ([None; 7], None),
        ];
        for (blockers, expected) in cases {
            assert_eq!(save_start_blocker(blockers), expected);
        }
        assert_eq!(
            save_start_blocker_message(FolderOpen),
            "Wait for the selected folder to finish opening before saving a copy"
        );
        assert_eq!(
            save_start_blocker_message(RatingWrite),
            "Wait for the rating update to finish before saving a copy"
        );
        assert_eq!(
            save_start_blocker_message(Recovery),
            crate::ui::SAVE_RECOVERY_STATUS
        );
    }

    #[test]
    fn deferred_close_requires_a_successful_save_terminal_state() {
        assert_eq!(
            save_close_disposition(false, SaveTerminalState::Succeeded, false),
            SaveCloseDisposition::StayOpen
        );
        assert_eq!(
            save_close_disposition(true, SaveTerminalState::Succeeded, false),
            SaveCloseDisposition::Exit
        );
        assert_eq!(
            save_close_disposition(true, SaveTerminalState::Succeeded, true),
            SaveCloseDisposition::WaitForCuration
        );
        for terminal in [SaveTerminalState::Failed, SaveTerminalState::Disconnected] {
            assert_eq!(
                save_close_disposition(true, terminal, false),
                SaveCloseDisposition::CancelDeferredClose
            );
            assert_eq!(
                save_close_disposition(true, terminal, true),
                SaveCloseDisposition::CancelDeferredClose
            );
        }
    }

    #[test]
    fn permanent_delete_confirmation_is_bounded_path_free_and_control_safe() {
        let path = PathBuf::from("private")
            .join("album")
            .join("bad\n\u{202e}\"gpj");
        let description = permanent_delete_description(&path);
        assert!(description.starts_with("Delete \"bad???gpj\" forever?"));
        assert!(!description.contains("private"));
        assert!(!description.contains("album"));
        assert!(!description.contains('\u{202e}'));
        assert_eq!(description.matches('\n').count(), 2);
        assert!(description.contains("system Trash"));

        let long_name = format!("{}.png", "a".repeat(140));
        let description = permanent_delete_description(Path::new(&long_name));
        let quoted = description
            .strip_prefix("Delete \"")
            .and_then(|value| value.split_once("\" forever?"))
            .map(|(name, _)| name)
            .expect("fixed confirmation structure");
        assert_eq!(quoted.chars().count(), 96);
    }

    #[test]
    fn permanent_delete_requires_the_explicit_destructive_action() {
        assert!(permanent_delete_confirmed(
            &rfd::MessageDialogResult::Custom(PERMANENT_DELETE_ACTION.to_owned())
        ));
        assert!(!permanent_delete_confirmed(&rfd::MessageDialogResult::Ok));
        assert!(!permanent_delete_confirmed(
            &rfd::MessageDialogResult::Custom("Cancel".to_owned())
        ));
    }

    #[test]
    fn source_bound_destructive_copy_is_exhaustive_and_path_free() {
        let trash_cases = [
            (
                GuardedActionError::Changed,
                "This file changed after it was displayed. Reload it before moving it to Trash. Nothing was moved.",
            ),
            (
                GuardedActionError::Missing,
                "This file is no longer available. Nothing was moved.",
            ),
            (
                GuardedActionError::Unsupported,
                "This filesystem entry cannot be safely moved from the displayed source. Nothing was moved.",
            ),
            (
                GuardedActionError::Unavailable,
                "Safe file identity could not be verified. Nothing was moved.",
            ),
            (
                GuardedActionError::OperationFailed("access denied".to_owned()),
                "Trash failed: access denied. Nothing was moved.",
            ),
        ];
        for (error, expected) in trash_cases {
            let message = guarded_action_failure_message(GuardedActionKind::Trash, &error);
            assert_eq!(message, expected);
            assert!(!message.contains("private"));
            assert!(!message.contains("album"));
        }

        let permanent_delete_cases = [
            (
                GuardedActionError::Changed,
                "This file changed after it was displayed. Reload it before deleting it. Nothing was deleted.",
            ),
            (
                GuardedActionError::Missing,
                "This file is no longer available. Nothing was deleted.",
            ),
            (
                GuardedActionError::Unsupported,
                "This filesystem entry cannot be safely deleted from the displayed source. Nothing was deleted.",
            ),
            (
                GuardedActionError::Unavailable,
                "Safe file identity could not be verified. Nothing was deleted.",
            ),
            (
                GuardedActionError::OperationFailed("access denied".to_owned()),
                "Delete failed: access denied. Nothing was deleted.",
            ),
        ];
        for (error, expected) in permanent_delete_cases {
            let message =
                guarded_action_failure_message(GuardedActionKind::PermanentDelete, &error);
            assert_eq!(message, expected);
            assert!(!message.contains("private"));
            assert!(!message.contains("album"));
        }
    }

    #[test]
    fn appearance_save_failure_copy_is_fixed_and_path_private() {
        let message = appearance_save_failure_message();
        assert_eq!(
            message,
            "Appearance changed for this session but could not be remembered. Check local configuration storage, then choose it again."
        );
        for private_fragment in ["C:\\Users\\private", "/home/private", "access denied"] {
            assert!(!message.contains(private_fragment));
        }
    }

    #[test]
    fn single_trash_copy_routes_every_move_to_a_real_recovery_path() {
        assert_eq!(
            single_trash_result_message(true, false),
            "Moved to Trash. Undo with U."
        );
        assert_eq!(
            single_trash_result_message(false, true),
            "Moved to Trash, but U is unavailable for this move. Use the system Trash; U still restores the previous Trash action."
        );
        assert_eq!(
            single_trash_result_message(false, false),
            "Moved to Trash, but U is unavailable for this move. Use the system Trash for recovery."
        );
    }

    #[test]
    fn permanent_delete_success_copy_disambiguates_prior_trash_undo() {
        let path = PathBuf::from("private")
            .join("album")
            .join("bad\n\u{202e}\"gpj");
        let with_prior = permanent_delete_success_message(&path, true);
        assert_eq!(
            with_prior,
            "Permanently deleted \"bad???gpj\". This cannot be undone; U still restores the previous Trash action."
        );
        assert!(!with_prior.contains("private"));
        assert!(!with_prior.contains("album"));
        assert!(!with_prior.contains('\n'));
        assert!(!with_prior.contains('\u{202e}'));

        assert_eq!(
            permanent_delete_success_message(&path, false),
            "Permanently deleted \"bad???gpj\". This cannot be undone."
        );
    }

    #[test]
    fn restore_copy_exposes_only_valid_retry_routes() {
        use crate::curate::TrashRestoreError as Error;

        let cases = [
            (
                Error::DestinationOccupied,
                "Restore blocked: The original folder already contains an item with that name. Move or rename it, then retry with U.",
            ),
            (
                Error::AccessDenied,
                "Restore blocked: Access was denied. Check permissions, then retry with U.",
            ),
            (
                Error::OperationFailed,
                "Restore failed: The operating system could not restore the file. Retry with U.",
            ),
            (
                Error::MissingFromTrash,
                "The exact item is no longer in the system Trash. No retry remains in viewr.",
            ),
            (
                Error::AmbiguousReceipt,
                "The exact Trash receipt is ambiguous. Use the system Trash; no retry remains in viewr.",
            ),
            (
                Error::Unsupported,
                "In-app restore is unsupported on this platform. Use the system Trash; no retry remains in viewr.",
            ),
            (
                Error::InvalidReceipt,
                "The exact Trash receipt is unavailable. Use the system Trash; no retry remains in viewr.",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(single_restore_failure_message(error), expected);
        }

        assert_eq!(
            restore_result_message(1, 0, 0, 0, 0, None, true),
            "Restored 1 file"
        );
        assert_eq!(
            restore_result_message(2, 0, 0, 0, 0, None, false),
            "Restored 2 files; reopen the source folder to refresh its view"
        );
        assert_eq!(
            restore_result_message(1, 1, 1, 1, 1, Some(Error::OperationFailed), true),
            "Restored 1 file; 1 file can retry with U; 1 file needs the blocking condition resolved, then U can retry; 1 file requires system Trash review; 1 file is no longer available for in-app restore."
        );
        let manual_only =
            restore_result_message(0, 0, 0, 1, 1, Some(Error::AmbiguousReceipt), true);
        assert_eq!(
            manual_only,
            "Nothing restored; 1 file requires system Trash review; 1 file is no longer available for in-app restore."
        );
        assert!(!manual_only.contains('U'));
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
    fn edit_transaction_copy_is_truthful_and_hides_internal_errors() {
        let edit_error = crate::heal::PatchPresentationError::<&str>::Edit(
            crate::heal::HealError::InvalidImageBuffer,
        );
        let edit_message = edit_transaction_failure_message("Undo", &edit_error, false);
        assert_eq!(
            edit_message,
            "Undo could not be applied. The image and edit history are unchanged."
        );
        assert!(!edit_message.contains("RGBA"));

        let rollback_error = crate::heal::PatchPresentationError::Rollback {
            presentation: "adapter rejected the update",
            rollback: crate::heal::HealError::InvalidPatch,
        };
        assert_eq!(
            edit_transaction_failure_message("Spot heal", &rollback_error, true),
            "Spot heal failed. Disk source unchanged; reloading it and clearing edit history."
        );
        assert_eq!(
            edit_transaction_failure_message("Spot heal", &rollback_error, false),
            "Spot heal failed. Disk source unchanged; reopen it. Edit history was cleared."
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
    fn crop_recovery_requires_generation_path_and_exact_source_allocation() {
        let path = PathBuf::from("album").join("source.png");
        let source_image = Arc::new(DecodedImage {
            rgba: vec![10, 20, 30, 255],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
        });
        let same_pixels_different_allocation = Arc::new(DecodedImage {
            rgba: source_image.rgba.clone(),
            width: source_image.width,
            height: source_image.height,
            color_profile: source_image.color_profile,
            working_color: source_image.working_color,
        });
        let transform = Transform {
            zoom: 2.5,
            offset_x: 0.2,
            offset_y: -0.1,
            rotation_steps: 1,
            flip_h: true,
            is_cropping: true,
            crop_rect: Some([0.2, 0.3, 0.8, 0.9]),
            ..Transform::default()
        };
        let recovery = CropRecovery {
            source_path: path.clone(),
            source_generation: 42,
            source_image: Arc::clone(&source_image),
            transform,
            animation: None,
            auxiliary_job: None,
        };

        assert!(crop_recovery_matches(
            &recovery,
            42,
            Some(&path),
            Some(&path),
            Some(&source_image),
        ));
        assert!(!crop_recovery_matches(
            &recovery,
            43,
            Some(&path),
            Some(&path),
            Some(&source_image),
        ));
        assert!(!crop_recovery_matches(
            &recovery,
            42,
            Some(Path::new("album/other.png")),
            Some(&path),
            Some(&source_image),
        ));
        assert!(!crop_recovery_matches(
            &recovery,
            42,
            Some(&path),
            Some(&path),
            Some(&same_pixels_different_allocation),
        ));
        assert_eq!(recovery.transform.crop_rect, Some([0.2, 0.3, 0.8, 0.9]));
        assert_eq!(recovery.transform.rotation_steps, 1);
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
            auxiliary_job,
        };
        let output = || {
            (
                Ok(None),
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
    fn crop_failure_copy_states_safe_state_and_direct_retry() {
        assert_eq!(
            crop_failure_message(true),
            "Crop was not applied. Original image unchanged; selection restored. Press Enter to try again."
        );
        assert_eq!(
            crop_failure_message(false),
            "Crop was not applied because the image changed."
        );
    }

    #[test]
    fn crop_disconnect_copy_requires_restart_without_promising_retry() {
        assert_eq!(
            crop_disconnect_message(true),
            "Crop stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_disconnect_message(false),
            "Crop stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
        );
    }

    #[test]
    fn crop_preview_disconnect_copy_and_recovery_priority_are_truthful() {
        assert_eq!(
            crop_preview_disconnect_message(true),
            "Crop could not finish because display preview preparation stopped unexpectedly. Original image unchanged; selection restored. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_preview_disconnect_message(false),
            "Display preview preparation stopped unexpectedly after the image changed. Close and reopen viewr before cropping again."
        );
        assert_eq!(
            crop_recovery_blocker(true, true),
            Some(crate::ui::CROP_RECOVERY_STATUS)
        );
        assert_eq!(
            crop_recovery_blocker(false, true),
            Some(crate::ui::PREVIEW_RECOVERY_STATUS)
        );
        assert_eq!(crop_recovery_blocker(false, false), None);
        assert_eq!(
            preview_retry_blocker(true),
            Some(crate::ui::PREVIEW_RECOVERY_STATUS)
        );
        assert_eq!(preview_retry_blocker(false), None);
    }

    #[test]
    fn crop_source_requires_a_settled_successful_image_load() {
        assert_eq!(
            crop_source_blocker(true, false),
            Some("Wait for the image to finish opening before cropping")
        );
        assert_eq!(
            crop_source_blocker(false, true),
            Some("Retry the failed image load before cropping")
        );
        assert_eq!(
            crop_source_blocker(true, true),
            Some("Wait for the image to finish opening before cropping")
        );
        assert_eq!(crop_source_blocker(false, false), None);
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
        for key in ["b", "B", "m", "M", "x", "X"] {
            assert!(
                !route_consumed_keyboard_key(&Key::Character(key.into()), false, false),
                "unused culling key {key} must not be intercepted"
            );
        }
        assert!(route_consumed_keyboard_key(
            &Key::Character("x".into()),
            true,
            false,
        ));
        assert!(is_trash_shortcut_key(&Key::Named(NamedKey::Delete)));
        assert_eq!(
            is_trash_shortcut_key(&Key::Named(NamedKey::Backspace)),
            cfg!(target_os = "macos")
        );
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
        for key in [
            NamedKey::Home,
            NamedKey::End,
            NamedKey::PageUp,
            NamedKey::PageDown,
        ] {
            assert!(route_consumed_keyboard_key(&Key::Named(key), false, false,));
        }
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

    #[test]
    fn selected_file_matches_relative_scan_results_by_name() {
        let files = vec![PathBuf::from("./img1.png"), PathBuf::from("./img2.png")];
        assert_eq!(
            selected_file_index_by(&files, Path::new("img2.png"), PathBuf::as_path),
            Some(1)
        );
        assert_eq!(
            selected_file_index_by(&files, Path::new("missing.png"), PathBuf::as_path),
            None
        );
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
    fn selected_prefetch_result_presents_only_while_pending_or_failed() {
        let selected = Path::new("selected.png");
        let playlist = Playlist::new(vec![selected.to_owned(), PathBuf::from("neighbor.png")], 0);

        assert_eq!(
            prefetch_destination(Some(selected), true, Some(&playlist), selected),
            PrefetchDestination::PresentSelected
        );
        assert_eq!(
            prefetch_destination(Some(selected), false, Some(&playlist), selected),
            PrefetchDestination::Ignore
        );
    }

    #[test]
    fn prefetch_result_caches_only_current_playlist_neighbors() {
        let selected = Path::new("selected.png");
        let neighbor = Path::new("neighbor.png");
        let playlist = Playlist::new(vec![selected.to_owned(), neighbor.to_owned()], 0);

        assert_eq!(
            prefetch_destination(Some(selected), false, Some(&playlist), neighbor),
            PrefetchDestination::CacheNeighbor
        );
        assert_eq!(
            prefetch_destination(
                Some(selected),
                true,
                Some(&playlist),
                Path::new("stale.png")
            ),
            PrefetchDestination::Ignore
        );
    }
}
