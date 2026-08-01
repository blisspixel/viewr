//! Bounded background thumbnail ownership for the filmstrip.
//!
//! The event loop owns every active job and is the sole authority for source
//! paths, playlist generations, and texture publication. Workers return only a
//! validated pixel payload or a path-free failure category.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::job::{JobPoll, OneShotJob};

/// Target edge length for filmstrip thumbnails (pixels).
pub const THUMB_EDGE: u32 = 72;

/// The filmstrip exposes at most nine cells around the current image.
pub(crate) const MAX_ACTIVE_THUMBNAILS: usize = 9;

/// A stable, path-free reason why a thumbnail could not be prepared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ThumbnailFailure {
    Decode,
    UnsupportedWorkingEncoding,
    EmptyImage,
    InvalidRgbaBuffer,
    InvalidOutput,
    WorkerPanicked,
    WorkerDisconnected,
}

impl ThumbnailFailure {
    /// Stable diagnostic text that cannot disclose a source path.
    pub(crate) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Decode => "decode failed",
            Self::UnsupportedWorkingEncoding => "unsupported working encoding",
            Self::EmptyImage => "empty image",
            Self::InvalidRgbaBuffer => "invalid decoded RGBA buffer",
            Self::InvalidOutput => "invalid thumbnail output",
            Self::WorkerPanicked => "worker panicked",
            Self::WorkerDisconnected => "worker disconnected",
        }
    }
}

/// Validated RGBA8 thumbnail ready for egui upload.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ThumbRgba {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl ThumbRgba {
    fn new(
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        working_color: crate::color::WorkingColorEncoding,
    ) -> Result<Self, ThumbnailFailure> {
        let expected_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4));
        if width == 0
            || height == 0
            || width > THUMB_EDGE
            || height > THUMB_EDGE
            || working_color != crate::color::WorkingColorEncoding::SRGB_RGBA8
            || expected_len != Some(rgba.len())
        {
            return Err(ThumbnailFailure::InvalidOutput);
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Pixel dimensions in the shape expected by egui.
    pub(crate) fn dimensions(&self) -> [usize; 2] {
        [self.width as usize, self.height as usize]
    }

    /// Borrow the tightly packed, unmultiplied RGBA8 rows.
    pub(crate) fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

#[derive(Debug)]
struct ThumbnailJobContext {
    path: PathBuf,
    generation: u64,
}

type ThumbnailResult = Result<ThumbRgba, ThumbnailFailure>;

const WAKE_PENDING: u8 = 0;
const WAKE_ARMED: u8 = 1;
const WAKE_SIGNALED: u8 = 2;
const WAKE_REJECTED: u8 = 3;
const WAKE_FIRED: u8 = 4;

/// Arms completion notification only after the bounded executor accepts work.
///
/// This handshake covers the race where a fast worker finishes before the
/// event loop inserts its owner. Rejected work stays silent even though dropping
/// its one-shot completion necessarily signals a disconnected endpoint.
struct CompletionWake<N: FnOnce()> {
    state: AtomicU8,
    notify: Mutex<Option<N>>,
}

impl<N: FnOnce()> CompletionWake<N> {
    fn new(notify: N) -> Self {
        Self {
            state: AtomicU8::new(WAKE_PENDING),
            notify: Mutex::new(Some(notify)),
        }
    }

    fn signal(&self) {
        match self.state.compare_exchange(
            WAKE_PENDING,
            WAKE_SIGNALED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(WAKE_SIGNALED | WAKE_REJECTED | WAKE_FIRED) => {}
            Err(WAKE_ARMED) => self.fire_from(WAKE_ARMED),
            Err(unexpected) => unreachable!("invalid completion wake state {unexpected}"),
        }
    }

    fn arm(&self) {
        match self.state.compare_exchange(
            WAKE_PENDING,
            WAKE_ARMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(WAKE_ARMED | WAKE_FIRED) => {}
            Err(WAKE_SIGNALED) => self.fire_from(WAKE_SIGNALED),
            Err(WAKE_REJECTED) => unreachable!("rejected thumbnail wake cannot be armed"),
            Err(unexpected) => unreachable!("invalid completion wake state {unexpected}"),
        }
    }

    fn reject(&self) {
        self.state.store(WAKE_REJECTED, Ordering::Release);
        self.notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    fn fire_from(&self, expected: u8) {
        if self
            .state
            .compare_exchange(expected, WAKE_FIRED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let notify = self
            .notify
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(notify) = notify {
            notify();
        }
    }
}

/// A current completion whose path came from event-loop-owned job context.
#[derive(Debug)]
pub(crate) enum ThumbnailCompletion {
    Ready { path: PathBuf, thumbnail: ThumbRgba },
    Failed { failure: ThumbnailFailure },
}

/// Results of one non-blocking pass over the active thumbnail jobs.
pub(crate) struct ThumbnailPoll {
    /// Current, visible completions that may affect presentation state.
    pub(crate) completions: Vec<ThumbnailCompletion>,
    /// Whether any active owner reached a terminal state, including stale work.
    pub(crate) made_progress: bool,
}

/// Event-loop-owned thumbnail generation, active jobs, and terminal failures.
#[derive(Default)]
pub(crate) struct ThumbnailSchedule {
    generation: u64,
    active: HashMap<PathBuf, OneShotJob<ThumbnailJobContext, ThumbnailResult>>,
    terminal_failures: HashSet<PathBuf>,
}

impl ThumbnailSchedule {
    /// Invalidate results from the current playlist or disk snapshot.
    ///
    /// Existing owners remain until workers finish so their terminal transition
    /// can wake the event loop and free shared decode capacity deterministically.
    pub(crate) fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.terminal_failures.clear();
    }

    /// Forget failure suppression after a path leaves the visible window.
    pub(crate) fn retain_visible_failures(&mut self, visible: &HashSet<PathBuf>) {
        self.terminal_failures.retain(|path| visible.contains(path));
    }

    /// Schedule one eligible path through a caller-supplied bounded executor.
    ///
    /// Saturation is non-terminal: the path remains eligible after another
    /// worker completion wakes the event loop. The worker always emits a wake,
    /// even when its owner has become stale, because queue capacity changed.
    pub(crate) fn request<N, S>(&mut self, path: PathBuf, notify: N, schedule: S) -> bool
    where
        N: FnOnce() + Send + 'static,
        S: FnOnce(Box<dyn FnOnce() + Send>) -> bool,
    {
        self.request_with(path, notify, schedule, generate_thumb)
    }

    fn request_with<N, S, W>(&mut self, path: PathBuf, notify: N, schedule: S, worker: W) -> bool
    where
        N: FnOnce() + Send + 'static,
        S: FnOnce(Box<dyn FnOnce() + Send>) -> bool,
        W: FnOnce(&Path) -> ThumbnailResult + Send + 'static,
    {
        if self.active.len() >= MAX_ACTIVE_THUMBNAILS
            || self.active.contains_key(&path)
            || self.terminal_failures.contains(&path)
        {
            return false;
        }

        let context = ThumbnailJobContext {
            path: path.clone(),
            generation: self.generation,
        };
        let wake = Arc::new(CompletionWake::new(notify));
        let completion_wake = Arc::clone(&wake);
        let (completion, owner) = OneShotJob::new(context, move || completion_wake.signal());
        let worker_path = path.clone();
        let task = Box::new(move || {
            let result = catch_unwind(AssertUnwindSafe(|| worker(&worker_path)))
                .unwrap_or(Err(ThumbnailFailure::WorkerPanicked));
            let _ = completion.complete(result);
        });
        if !schedule(task) {
            wake.reject();
            return false;
        }

        let replaced = self.active.insert(path, owner);
        debug_assert!(replaced.is_none(), "eligible thumbnail path must be unique");
        wake.arm();
        true
    }

    /// Poll every active owner once and return only current visible results.
    pub(crate) fn poll(&mut self, visible: &HashSet<PathBuf>) -> ThumbnailPoll {
        let mut terminal = Vec::new();
        for (path, job) in &self.active {
            match job.poll() {
                JobPoll::Pending => {}
                JobPoll::Ready(result) => terminal.push((path.clone(), Some(result))),
                JobPoll::Disconnected => terminal.push((path.clone(), None)),
            }
        }

        let made_progress = !terminal.is_empty();
        let mut completions = Vec::with_capacity(terminal.len());
        for (key, result) in terminal {
            let owner = self
                .active
                .remove(&key)
                .expect("polled thumbnail owner remains active until collection");
            let context = owner.into_context();
            if context.generation != self.generation || !visible.contains(&context.path) {
                continue;
            }
            match result {
                Some(Ok(thumbnail)) => completions.push(ThumbnailCompletion::Ready {
                    path: context.path,
                    thumbnail,
                }),
                Some(Err(failure)) => {
                    self.terminal_failures.insert(context.path.clone());
                    completions.push(ThumbnailCompletion::Failed { failure });
                }
                None => {
                    self.terminal_failures.insert(context.path.clone());
                    completions.push(ThumbnailCompletion::Failed {
                        failure: ThumbnailFailure::WorkerDisconnected,
                    });
                }
            }
        }
        ThumbnailPoll {
            completions,
            made_progress,
        }
    }

    /// Number of event-loop-owned jobs, including stale jobs awaiting cleanup.
    pub(crate) fn in_flight_len(&self) -> usize {
        self.active.len()
    }

    /// Whether every scheduled owner has reached a terminal state.
    pub(crate) fn is_idle(&self) -> bool {
        self.active.is_empty()
    }

    /// Whether the visible path has a stable placeholder for this generation.
    pub(crate) fn has_terminal_failure(&self, path: &Path) -> bool {
        self.terminal_failures.contains(path)
    }
}

/// Decode `path` and downscale to fit inside a [`THUMB_EDGE`] box.
fn generate_thumb(path: &Path) -> ThumbnailResult {
    let decoded =
        crate::decode::DecodedImage::load_background(path).map_err(|_| ThumbnailFailure::Decode)?;
    resize_decoded(decoded)
}

fn resize_decoded(decoded: crate::decode::DecodedImage) -> ThumbnailResult {
    if decoded.working_color != crate::color::WorkingColorEncoding::SRGB_RGBA8 {
        return Err(ThumbnailFailure::UnsupportedWorkingEncoding);
    }
    if decoded.width == 0 || decoded.height == 0 {
        return Err(ThumbnailFailure::EmptyImage);
    }
    let img = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
        .ok_or(ThumbnailFailure::InvalidRgbaBuffer)?;

    let (width, height) = fit_size(decoded.width, decoded.height, THUMB_EDGE);
    let resized =
        image::imageops::resize(&img, width, height, image::imageops::FilterType::Triangle);
    ThumbRgba::new(
        resized.width(),
        resized.height(),
        resized.into_raw(),
        decoded.working_color,
    )
}

/// Scale `(w, h)` to fit inside a square of `edge` while preserving aspect ratio.
#[must_use]
pub fn fit_size(w: u32, h: u32, edge: u32) -> (u32, u32) {
    if w == 0 || h == 0 || edge == 0 {
        return (1, 1);
    }
    let wf = f64::from(w);
    let hf = f64::from(h);
    let e = f64::from(edge);
    let scale = (e / wf).min(e / hf).min(1.0);
    let tw = f64_to_px((wf * scale).round().max(1.0));
    let th = f64_to_px((hf * scale).round().max(1.0));
    (tw, th)
}

fn f64_to_px(v: f64) -> u32 {
    let v = v.clamp(1.0, f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        v as u32
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::{
        MAX_ACTIVE_THUMBNAILS, THUMB_EDGE, ThumbRgba, ThumbnailCompletion, ThumbnailFailure,
        ThumbnailSchedule, fit_size, generate_thumb, resize_decoded,
    };

    fn valid_thumbnail() -> ThumbRgba {
        ThumbRgba::new(
            2,
            2,
            vec![0; 16],
            crate::color::WorkingColorEncoding::SRGB_RGBA8,
        )
        .unwrap()
    }

    fn visible(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn fit_size_preserves_aspect_and_caps_edge() {
        assert_eq!(fit_size(200, 100, 50), (50, 25));
        assert_eq!(fit_size(100, 200, 50), (25, 50));
        assert_eq!(fit_size(20, 10, 50), (20, 10));
        assert_eq!(fit_size(0, 50, THUMB_EDGE), (1, 1));
    }

    #[test]
    fn generate_thumb_from_png() {
        let ws = crate::ephemeral::TempWorkspace::new("thumb").unwrap();
        let path = ws.path().join("big.png");
        image::RgbImage::from_fn(120, 80, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 40])
        })
        .save(&path)
        .unwrap();

        let thumb = generate_thumb(&path).expect("thumb");

        assert!(thumb.dimensions()[0] <= THUMB_EDGE as usize);
        assert!(thumb.dimensions()[1] <= THUMB_EDGE as usize);
        assert_eq!(
            thumb.rgba().len(),
            thumb.dimensions()[0] * thumb.dimensions()[1] * 4
        );
    }

    #[test]
    fn thumbnail_rejects_an_unsupported_working_encoding() {
        let decoded = crate::decode::DecodedImage {
            rgba: vec![0; 16],
            width: 2,
            height: 2,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: crate::color::WorkingColorEncoding::DISPLAY_P3_RGBA8,
        };

        assert_eq!(
            resize_decoded(decoded),
            Err(ThumbnailFailure::UnsupportedWorkingEncoding)
        );
    }

    #[test]
    fn thumbnail_rejects_empty_dimensions_before_resizing() {
        let decoded = crate::decode::DecodedImage {
            rgba: Vec::new(),
            width: 0,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: crate::color::WorkingColorEncoding::SRGB_RGBA8,
        };

        assert_eq!(resize_decoded(decoded), Err(ThumbnailFailure::EmptyImage));
    }

    #[test]
    fn thumbnail_payload_rejects_invalid_shape_bytes_and_encoding() {
        let srgb = crate::color::WorkingColorEncoding::SRGB_RGBA8;
        assert_eq!(
            ThumbRgba::new(0, 1, Vec::new(), srgb),
            Err(ThumbnailFailure::InvalidOutput)
        );
        assert_eq!(
            ThumbRgba::new(
                THUMB_EDGE + 1,
                1,
                vec![0; (THUMB_EDGE as usize + 1) * 4],
                srgb
            ),
            Err(ThumbnailFailure::InvalidOutput)
        );
        assert_eq!(
            ThumbRgba::new(2, 2, vec![0; 15], srgb),
            Err(ThumbnailFailure::InvalidOutput)
        );
        assert_eq!(
            ThumbRgba::new(
                2,
                2,
                vec![0; 16],
                crate::color::WorkingColorEncoding::DISPLAY_P3_RGBA8,
            ),
            Err(ThumbnailFailure::InvalidOutput)
        );
    }

    #[test]
    fn schedule_caps_owners_and_does_not_own_rejected_work() {
        let mut schedule = ThumbnailSchedule::default();
        let queued = Arc::new(Mutex::new(Vec::new()));
        for index in 0..MAX_ACTIVE_THUMBNAILS {
            let queued = Arc::clone(&queued);
            assert!(schedule.request_with(
                PathBuf::from(format!("{index}.png")),
                || {},
                move |task| {
                    queued.lock().unwrap().push(task);
                    true
                },
                |_| Ok(valid_thumbnail()),
            ));
        }
        assert_eq!(schedule.in_flight_len(), MAX_ACTIVE_THUMBNAILS);
        assert!(!schedule.request_with(
            PathBuf::from("overflow.png"),
            || {},
            |_| true,
            |_| Ok(valid_thumbnail()),
        ));

        let rejected_notifications = Arc::new(AtomicUsize::new(0));
        let rejected_notified = Arc::clone(&rejected_notifications);
        let mut rejected = ThumbnailSchedule::default();
        assert!(!rejected.request_with(
            PathBuf::from("retry.png"),
            move || {
                rejected_notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                drop(task);
                false
            },
            |_| Ok(valid_thumbnail()),
        ));
        assert!(rejected.is_idle());
        assert_eq!(rejected_notifications.load(Ordering::Acquire), 0);
    }

    #[test]
    fn old_generation_completion_cannot_publish_for_same_visible_path() {
        let mut schedule = ThumbnailSchedule::default();
        let queued = Arc::new(Mutex::new(Vec::new()));
        let queued_worker = Arc::clone(&queued);
        assert!(schedule.request_with(
            PathBuf::from("same.png"),
            || {},
            move |task| {
                queued_worker.lock().unwrap().push(task);
                true
            },
            |_| Ok(valid_thumbnail()),
        ));
        schedule.reset();
        let task = queued.lock().unwrap().pop().unwrap();
        task();

        let poll = schedule.poll(&visible(&["same.png"]));

        assert!(poll.made_progress);
        assert!(poll.completions.is_empty());
        assert!(schedule.is_idle());
    }

    #[test]
    fn off_window_completion_is_rejected_but_still_reports_progress() {
        let mut schedule = ThumbnailSchedule::default();
        assert!(schedule.request_with(
            PathBuf::from("old.png"),
            || {},
            |task| {
                task();
                true
            },
            |_| Ok(valid_thumbnail()),
        ));

        let poll = schedule.poll(&visible(&["new.png"]));

        assert!(poll.made_progress);
        assert!(poll.completions.is_empty());
    }

    #[test]
    fn failure_is_terminal_only_while_path_remains_visible() {
        let mut schedule = ThumbnailSchedule::default();
        let path = PathBuf::from("broken.png");
        assert!(schedule.request_with(
            path.clone(),
            || {},
            |task| {
                task();
                true
            },
            |_| Err(ThumbnailFailure::Decode),
        ));

        let poll = schedule.poll(&visible(&["broken.png"]));

        assert!(matches!(
            poll.completions.as_slice(),
            [ThumbnailCompletion::Failed {
                failure: ThumbnailFailure::Decode,
            }]
        ));
        assert!(schedule.has_terminal_failure(&path));
        assert!(!schedule.request_with(path.clone(), || {}, |_| true, |_| Ok(valid_thumbnail()),));

        schedule.retain_visible_failures(&HashSet::new());
        assert!(!schedule.has_terminal_failure(&path));
        assert!(schedule.request_with(
            path,
            || {},
            |task| {
                task();
                true
            },
            |_| Ok(valid_thumbnail()),
        ));
    }

    #[test]
    fn generation_reset_clears_terminal_failure_suppression() {
        let mut schedule = ThumbnailSchedule::default();
        let path = PathBuf::from("retry-after-reset.png");
        assert!(schedule.request_with(
            path.clone(),
            || {},
            |task| {
                task();
                true
            },
            |_| Err(ThumbnailFailure::Decode),
        ));
        let failure = schedule.poll(&visible(&["retry-after-reset.png"]));
        assert!(matches!(
            failure.completions.as_slice(),
            [ThumbnailCompletion::Failed {
                failure: ThumbnailFailure::Decode,
            }]
        ));

        schedule.reset();

        assert!(!schedule.has_terminal_failure(&path));
        assert!(schedule.request_with(
            path,
            || {},
            |task| {
                task();
                true
            },
            |_| Ok(valid_thumbnail()),
        ));
    }

    #[test]
    fn dropped_worker_and_panicking_worker_are_observable_terminal_failures() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let mut schedule = ThumbnailSchedule::default();
        assert!(schedule.request_with(
            PathBuf::from("panic.png"),
            move || {
                notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                task();
                true
            },
            |_| panic!("thumbnail test panic"),
        ));
        let panic_poll = schedule.poll(&visible(&["panic.png"]));
        assert!(matches!(
            panic_poll.completions.as_slice(),
            [ThumbnailCompletion::Failed {
                failure: ThumbnailFailure::WorkerPanicked,
            }]
        ));
        assert_eq!(notifications.load(Ordering::Acquire), 1);

        let disconnected_notifications = Arc::new(AtomicUsize::new(0));
        let disconnected_notified = Arc::clone(&disconnected_notifications);
        assert!(schedule.request_with(
            PathBuf::from("dropped.png"),
            move || {
                disconnected_notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                drop(task);
                true
            },
            |_| Ok(valid_thumbnail()),
        ));
        let disconnected = schedule.poll(&visible(&["dropped.png"]));
        assert!(matches!(
            disconnected.completions.as_slice(),
            [ThumbnailCompletion::Failed {
                failure: ThumbnailFailure::WorkerDisconnected,
            }]
        ));
        assert_eq!(disconnected_notifications.load(Ordering::Acquire), 1);
    }

    #[test]
    fn completion_path_comes_from_owner_context() {
        let mut schedule = ThumbnailSchedule::default();
        let path = PathBuf::from("authoritative.png");
        assert!(schedule.request_with(
            path.clone(),
            || {},
            |task| {
                task();
                true
            },
            |_| Ok(valid_thumbnail()),
        ));

        let poll = schedule.poll(&visible(&["authoritative.png"]));

        assert!(matches!(
            poll.completions.as_slice(),
            [ThumbnailCompletion::Ready {
                path: completed_path,
                ..
            }] if completed_path == &path
        ));
    }
}
