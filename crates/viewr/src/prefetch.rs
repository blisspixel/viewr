//! Bounded in-memory neighbor decode cache (privacy-safe: never on disk).
//!
//! When the user arrows through a folder, next/previous images should already
//! be decoded in RAM. This is an LRU of [`DecodedImage`] keyed by path, with no
//! thumbnail database, no history file, cleared when the process exits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::decode::{DecodedImage, LoadedImage};
use crate::job::{JobPoll, OneShotJob, try_schedule_one_shot};

/// Default number of decoded full images kept around the current one.
pub const DEFAULT_CAPACITY: usize = 5;
/// Default decoded-neighbor RAM budget: 256 MiB.
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_SAFE_FILENAME_CHARS: usize = 96;
const MAX_ACTIVE_JOBS: usize = 4;

/// Stable, path-free failure categories for speculative decode diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrefetchFailure {
    /// The checked image decoder rejected the source.
    Decode,
    /// A worker unwound before publishing a result in unwind-enabled builds.
    WorkerPanicked,
    /// The accepted executor closure was dropped without running.
    WorkerDisconnected,
}

impl PrefetchFailure {
    /// Privacy-safe diagnostic category suitable for local logs.
    #[must_use]
    pub(crate) const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::Decode => "decode failed",
            Self::WorkerPanicked => "worker panicked",
            Self::WorkerDisconnected => "worker disconnected",
        }
    }
}

type PrefetchResult = Result<Option<LoadedImage>, PrefetchFailure>;

/// Event-loop-owned identity and cancellation state for one speculative decode.
#[derive(Debug)]
struct PrefetchJobContext {
    path: PathBuf,
    generation: u64,
    cancellation: Arc<AtomicU64>,
    foreground_proven: bool,
}

/// One current-generation completion whose path came from owner context.
pub(crate) struct PrefetchCompletion {
    path: PathBuf,
    result: PrefetchResult,
    foreground_proven: bool,
}

impl PrefetchCompletion {
    /// Consume the completion for event-loop destination and terminal policy.
    pub(crate) fn into_parts(self) -> (PathBuf, PrefetchResult, bool) {
        (self.path, self.result, self.foreground_proven)
    }
}

/// Non-blocking collection of all terminal owners observed in one poll.
pub(crate) struct PrefetchPoll {
    completions: Vec<PrefetchCompletion>,
    made_progress: bool,
}

impl PrefetchPoll {
    /// Whether any owner completed, disconnected, or was discarded as stale.
    #[must_use]
    pub(crate) const fn made_progress(&self) -> bool {
        self.made_progress
    }

    /// Consume the poll and return publishable current-generation completions.
    #[must_use]
    pub(crate) fn into_completions(self) -> Vec<PrefetchCompletion> {
        self.completions
    }
}

/// Per-playlist scheduling state for speculative neighbor decodes.
///
/// Paths with terminal outcomes remain suppressed until the playlist changes or
/// a successful foreground presentation proves that the path is usable again.
#[derive(Default)]
pub(crate) struct PrefetchSchedule {
    generation: u64,
    active: Vec<OneShotJob<PrefetchJobContext, PrefetchResult>>,
    terminal: HashSet<PathBuf>,
}

impl PrefetchSchedule {
    /// Whether a path can start a speculative decode in the current generation.
    #[must_use]
    pub(crate) fn is_eligible(&self, path: &Path) -> bool {
        self.active.len() < MAX_ACTIVE_JOBS
            && !self.terminal.contains(path)
            && !self.active.iter().any(|job| {
                job.context().generation == self.generation && job.context().path == path
            })
    }

    /// Submit one bounded speculative decode and own its only result endpoint.
    ///
    /// Returns `false` without retaining state when the path is ineligible or
    /// the shared bounded executor rejects the closure.
    pub(crate) fn request<N, S>(&mut self, path: PathBuf, notify: N, schedule: S) -> bool
    where
        N: FnOnce() + Send + 'static,
        S: FnOnce(Box<dyn FnOnce() + Send>) -> bool,
    {
        self.request_with(path, notify, schedule, |path, cancellation| {
            DecodedImage::load_background_if_current(path, cancellation, 0)
                .map_err(|_| PrefetchFailure::Decode)
        })
    }

    fn request_with<N, S, W>(&mut self, path: PathBuf, notify: N, schedule: S, worker: W) -> bool
    where
        N: FnOnce() + Send + 'static,
        S: FnOnce(Box<dyn FnOnce() + Send>) -> bool,
        W: FnOnce(&Path, &AtomicU64) -> PrefetchResult + Send + 'static,
    {
        if !self.is_eligible(&path) {
            return false;
        }
        let cancellation = Arc::new(AtomicU64::new(0));
        let context = PrefetchJobContext {
            path: path.clone(),
            generation: self.generation,
            cancellation: Arc::clone(&cancellation),
            foreground_proven: false,
        };
        let worker_path = path;
        try_schedule_one_shot(
            context,
            notify,
            schedule,
            move || {
                catch_unwind(AssertUnwindSafe(|| worker(&worker_path, &cancellation)))
                    .unwrap_or(Err(PrefetchFailure::WorkerPanicked))
            },
            |owner| self.active.push(owner),
        )
    }

    /// Poll all owner endpoints once without blocking the event loop.
    #[must_use]
    pub(crate) fn poll(&mut self) -> PrefetchPoll {
        let mut completions = Vec::new();
        let mut made_progress = false;
        let mut index = 0;
        while index < self.active.len() {
            let result = match self.active[index].poll() {
                JobPoll::Pending => {
                    index += 1;
                    continue;
                }
                JobPoll::Ready(result) => result,
                JobPoll::Disconnected => Err(PrefetchFailure::WorkerDisconnected),
            };
            let job = self.active.swap_remove(index);
            let context = job.into_context();
            made_progress = true;
            if context.generation == self.generation {
                completions.push(PrefetchCompletion {
                    path: context.path,
                    result,
                    foreground_proven: context.foreground_proven,
                });
            }
        }

        PrefetchPoll {
            completions,
            made_progress,
        }
    }

    /// Record the event-loop policy outcome for a collected current result.
    ///
    /// Returns whether this completion created an effective terminal transition
    /// after accounting for a trusted foreground presentation.
    pub(crate) fn record_outcome(
        &mut self,
        path: &Path,
        terminal: bool,
        foreground_proven: bool,
    ) -> bool {
        let effective_terminal = terminal && !foreground_proven;
        if effective_terminal {
            self.terminal.insert(path.to_owned());
        }
        effective_terminal
    }

    /// Start a new playlist generation and cancel prior speculative work.
    ///
    /// Stale owners remain bounded and observable until their workers publish or
    /// disconnect. Their results can never enter the new generation.
    pub(crate) fn reset(&mut self) {
        for job in &self.active {
            job.context().cancellation.store(1, Ordering::Release);
        }
        self.generation = self.generation.wrapping_add(1);
        self.terminal.clear();
    }

    /// Allow a path again after a successful trusted presentation.
    pub(crate) fn allow(&mut self, path: &Path) {
        self.terminal.remove(path);
        if let Some(job) = self
            .active
            .iter_mut()
            .find(|job| job.context().generation == self.generation && job.context().path == path)
        {
            job.context().cancellation.store(1, Ordering::Release);
            job.context_mut().foreground_proven = true;
        }
    }

    /// Number of accepted jobs whose owner endpoints are still active.
    #[must_use]
    pub(crate) fn in_flight_len(&self) -> usize {
        self.active.len()
    }

    /// Whether no accepted speculative job remains active.
    #[must_use]
    pub(crate) fn is_idle(&self) -> bool {
        self.active.is_empty()
    }
}

/// Privacy-safe filename for UI status and opt-in diagnostics.
///
/// Directories are removed, control characters are replaced, and output is
/// bounded. Non-Unicode names use a stable placeholder.
#[must_use]
pub fn privacy_safe_file_name(path: &Path) -> String {
    let Some(name) = path.file_name() else {
        return "<unknown filename>".into();
    };
    let Some(name) = name.to_str() else {
        return "<non-Unicode filename>".into();
    };
    let name = name
        .chars()
        .take(MAX_SAFE_FILENAME_CHARS)
        .map(|character| {
            if filename_character_is_unsafe(character) {
                '?'
            } else {
                character
            }
        })
        .collect::<String>();
    if name.is_empty() {
        "<empty filename>".into()
    } else {
        name
    }
}

fn filename_character_is_unsafe(character: char) -> bool {
    character.is_control()
        || (character.is_whitespace() && character != ' ')
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{206f}'
        )
}

/// LRU cache of fully decoded images for instant navigation.
pub struct PrefetchCache {
    capacity: usize,
    max_bytes: usize,
    current_bytes: usize,
    order: VecDeque<PathBuf>,
    images: HashMap<PathBuf, Arc<DecodedImage>>,
}

impl Default for PrefetchCache {
    fn default() -> Self {
        Self::with_limits(DEFAULT_CAPACITY, DEFAULT_MAX_BYTES)
    }
}

impl PrefetchCache {
    /// Create an entry-bounded cache using the default byte budget.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self::with_limits(capacity, DEFAULT_MAX_BYTES)
    }

    /// Create a cache bounded by both entry count and decoded RGBA bytes.
    #[must_use]
    pub fn with_limits(capacity: usize, max_bytes: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            max_bytes,
            current_bytes: 0,
            order: VecDeque::new(),
            images: HashMap::new(),
        }
    }

    /// Number of cached images.
    #[must_use]
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Total decoded pixel bytes currently retained.
    #[must_use]
    pub const fn bytes(&self) -> usize {
        self.current_bytes
    }

    /// True when empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Look up a path without changing LRU order.
    #[must_use]
    pub fn peek(&self, path: &Path) -> Option<&DecodedImage> {
        self.images.get(path).map(Arc::as_ref)
    }

    /// Take shared ownership of a cached image (removes it from the cache).
    pub fn take(&mut self, path: &Path) -> Option<Arc<DecodedImage>> {
        let img = self.images.remove(path)?;
        self.current_bytes = self.current_bytes.saturating_sub(img.rgba.len());
        self.order.retain(|p| p != path);
        Some(img)
    }

    /// Insert or replace, evicting least-recently-used entries until both
    /// limits hold. Returns whether the new image remains cached. An image larger
    /// than the complete byte budget is not cached.
    pub fn insert(&mut self, path: PathBuf, image: impl Into<Arc<DecodedImage>>) -> bool {
        if let Some(replaced) = self.images.remove(&path) {
            self.current_bytes = self.current_bytes.saturating_sub(replaced.rgba.len());
            self.order.retain(|p| p != &path);
        }
        let image = image.into();
        let image_bytes = image.rgba.len();
        if image_bytes > self.max_bytes {
            return false;
        }
        let retained_path = path.clone();
        self.order.push_back(path.clone());
        self.current_bytes = self.current_bytes.saturating_add(image_bytes);
        self.images.insert(path, image);
        while self.images.len() > self.capacity || self.current_bytes > self.max_bytes {
            if let Some(old) = self.order.pop_front() {
                if let Some(evicted) = self.images.remove(&old) {
                    self.current_bytes = self.current_bytes.saturating_sub(evicted.rgba.len());
                }
            } else {
                break;
            }
        }
        self.images.contains_key(&retained_path)
    }

    /// True if this path is already fully decoded in the cache.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.images.contains_key(path)
    }

    /// Drop everything (e.g. when the folder playlist is replaced).
    pub fn clear(&mut self) {
        self.order.clear();
        self.images.clear();
        self.current_bytes = 0;
    }
}

/// Indices around `current` to prefetch (prev/next, then ±2), clamped to `len`.
///
/// Does not include `current` itself. Stable order: nearer neighbors first.
#[must_use]
pub fn neighbor_indices(current: usize, len: usize, radius: usize) -> Vec<usize> {
    if len == 0 || radius == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in 1..=radius {
        if let Some(i) = current.checked_sub(d) {
            out.push(i);
        }
        let next = current + d;
        if next < len {
            out.push(next);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_ACTIVE_JOBS, PrefetchCache, PrefetchFailure, PrefetchSchedule, neighbor_indices,
        privacy_safe_file_name,
    };
    use crate::color::WorkingColorEncoding;
    use crate::decode::DecodedImage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn tiny(id: u8) -> DecodedImage {
        DecodedImage {
            rgba: vec![id, 0, 0, 255],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        }
    }

    fn bytes(id: u8, len: usize) -> DecodedImage {
        DecodedImage {
            rgba: vec![id; len],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
            working_color: WorkingColorEncoding::SRGB_RGBA8,
        }
    }

    #[test]
    fn neighbor_indices_near_edges() {
        assert_eq!(neighbor_indices(0, 5, 2), vec![1, 2]);
        assert_eq!(neighbor_indices(4, 5, 2), vec![3, 2]);
        assert_eq!(neighbor_indices(2, 5, 1), vec![1, 3]);
        assert!(neighbor_indices(0, 0, 2).is_empty());
    }

    #[test]
    fn lru_evicts_oldest() {
        let mut c = PrefetchCache::with_capacity(2);
        c.insert(PathBuf::from("a"), tiny(1));
        c.insert(PathBuf::from("b"), tiny(2));
        c.insert(PathBuf::from("c"), tiny(3));
        assert_eq!(c.len(), 2);
        assert!(!c.contains(std::path::Path::new("a")));
        assert!(c.contains(std::path::Path::new("b")));
        assert!(c.contains(std::path::Path::new("c")));
    }

    #[test]
    fn take_removes_entry() {
        let mut c = PrefetchCache::with_capacity(3);
        c.insert(PathBuf::from("a"), tiny(9));
        let img = c.take(std::path::Path::new("a")).unwrap();
        assert_eq!(img.rgba[0], 9);
        assert!(c.is_empty());
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn take_returns_the_same_shared_decode_without_copying_pixels() {
        let mut cache = PrefetchCache::with_limits(3, 16);
        let image = Arc::new(bytes(7, 8));

        assert!(cache.insert(PathBuf::from("a"), Arc::clone(&image)));
        assert_eq!(cache.bytes(), 8);
        let taken = cache.take(std::path::Path::new("a")).unwrap();

        assert!(Arc::ptr_eq(&taken, &image));
        assert_eq!(taken.rgba.as_ptr(), image.rgba.as_ptr());
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn taking_the_selected_alias_restores_unique_edit_ownership() {
        let mut cache = PrefetchCache::with_limits(3, 16);
        let mut displayed = Arc::new(bytes(4, 8));
        assert!(cache.insert(PathBuf::from("a"), Arc::clone(&displayed)));
        assert!(Arc::get_mut(&mut displayed).is_none());

        drop(cache.take(std::path::Path::new("a")));

        assert!(Arc::get_mut(&mut displayed).is_some());
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn byte_budget_evicts_oldest_even_below_entry_capacity() {
        let mut cache = PrefetchCache::with_limits(5, 10);
        cache.insert(PathBuf::from("a"), bytes(1, 6));
        cache.insert(PathBuf::from("b"), bytes(2, 6));
        assert!(!cache.contains(std::path::Path::new("a")));
        assert!(cache.contains(std::path::Path::new("b")));
        assert_eq!(cache.bytes(), 6);
    }

    #[test]
    fn single_image_larger_than_budget_is_not_retained() {
        let mut cache = PrefetchCache::with_limits(5, 4);
        assert!(!cache.insert(PathBuf::from("large"), bytes(1, 5)));
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }

    #[test]
    fn schedule_caps_owners_and_rejected_work_stays_retryable() {
        let mut schedule = PrefetchSchedule::default();
        let queued = Arc::new(Mutex::new(Vec::new()));
        for index in 0..MAX_ACTIVE_JOBS {
            let queued = Arc::clone(&queued);
            assert!(schedule.request_with(
                PathBuf::from(format!("{index}.png")),
                || {},
                move |task| {
                    queued.lock().unwrap().push(task);
                    true
                },
                |_, _| Ok(None),
            ));
        }
        assert_eq!(schedule.in_flight_len(), MAX_ACTIVE_JOBS);
        assert!(!schedule.request_with(
            PathBuf::from("overflow.png"),
            || {},
            |_| true,
            |_, _| Ok(None),
        ));

        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let mut rejected = PrefetchSchedule::default();
        let path = PathBuf::from("retry.png");
        assert!(!rejected.request_with(
            path.clone(),
            move || {
                notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                drop(task);
                false
            },
            |_, _| Ok(None),
        ));
        assert!(rejected.is_idle());
        assert!(rejected.is_eligible(&path));
        assert_eq!(notifications.load(Ordering::Acquire), 0);
    }

    #[test]
    fn production_request_maps_decoder_errors_to_a_stable_path_free_category() {
        let workspace = crate::ephemeral::TempWorkspace::new("prefetch-missing").unwrap();
        let missing = workspace.path().join("private-missing.png");
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = Arc::clone(&notifications);
        let mut schedule = PrefetchSchedule::default();

        assert!(schedule.request(
            missing,
            move || {
                notified.fetch_add(1, Ordering::AcqRel);
            },
            |task| {
                task();
                true
            },
        ));

        assert_eq!(notifications.load(Ordering::Acquire), 1);
        let completion = schedule.poll().into_completions().pop().unwrap();
        let (_, result, _) = completion.into_parts();
        assert!(matches!(result, Err(PrefetchFailure::Decode)));
        assert_eq!(PrefetchFailure::Decode.diagnostic_name(), "decode failed");
        assert_eq!(
            PrefetchFailure::WorkerPanicked.diagnostic_name(),
            "worker panicked"
        );
        assert_eq!(
            PrefetchFailure::WorkerDisconnected.diagnostic_name(),
            "worker disconnected"
        );
    }

    #[test]
    fn stale_completion_cannot_publish_for_same_path_in_new_generation() {
        let mut schedule = PrefetchSchedule::default();
        let queued = Arc::new(Mutex::new(Vec::new()));
        let path = PathBuf::from("shared.png");
        let old_cancelled = Arc::new(AtomicBool::new(false));
        let observed_old_cancel = Arc::clone(&old_cancelled);
        let old_queue = Arc::clone(&queued);
        assert!(schedule.request_with(
            path.clone(),
            || {},
            move |task| {
                old_queue.lock().unwrap().push(task);
                true
            },
            move |_, cancellation| {
                observed_old_cancel
                    .store(cancellation.load(Ordering::Acquire) != 0, Ordering::Release);
                Err(PrefetchFailure::Decode)
            },
        ));
        schedule.reset();
        let current_queue = Arc::clone(&queued);
        assert!(schedule.request_with(
            path.clone(),
            || {},
            move |task| {
                current_queue.lock().unwrap().push(task);
                true
            },
            |_, _| Ok(None),
        ));

        let old_task = queued.lock().unwrap().remove(0);
        old_task();
        let stale = schedule.poll();
        assert!(stale.made_progress());
        assert!(stale.into_completions().is_empty());
        assert!(old_cancelled.load(Ordering::Acquire));
        assert_eq!(schedule.in_flight_len(), 1);

        let current_task = queued.lock().unwrap().remove(0);
        current_task();
        let mut current = schedule.poll().into_completions();
        assert_eq!(current.len(), 1);
        let (completed_path, result, superseded) = current.pop().unwrap().into_parts();
        assert_eq!(completed_path, path);
        assert!(matches!(result, Ok(None)));
        assert!(!superseded);
        assert!(schedule.is_idle());
    }

    #[test]
    fn foreground_success_cancels_and_supersedes_only_the_same_current_job() {
        let mut schedule = PrefetchSchedule::default();
        let selected = PathBuf::from("selected.png");
        let neighbor = PathBuf::from("neighbor.png");
        let queued = Arc::new(Mutex::new(Vec::new()));
        let selected_cancelled = Arc::new(AtomicBool::new(false));
        let selected_observation = Arc::clone(&selected_cancelled);
        let worker_queue = Arc::clone(&queued);
        assert!(schedule.request_with(
            selected.clone(),
            || {},
            move |task| {
                worker_queue.lock().unwrap().push(task);
                true
            },
            move |_, cancellation| {
                selected_observation
                    .store(cancellation.load(Ordering::Acquire) != 0, Ordering::Release);
                Err(PrefetchFailure::Decode)
            },
        ));
        let neighbor_cancelled = Arc::new(AtomicBool::new(false));
        let neighbor_observation = Arc::clone(&neighbor_cancelled);
        let neighbor_queue = Arc::clone(&queued);
        assert!(schedule.request_with(
            neighbor.clone(),
            || {},
            move |task| {
                neighbor_queue.lock().unwrap().push(task);
                true
            },
            move |_, cancellation| {
                neighbor_observation
                    .store(cancellation.load(Ordering::Acquire) != 0, Ordering::Release);
                Ok(None)
            },
        ));

        schedule.allow(&selected);
        while let Some(task) = queued.lock().unwrap().pop() {
            task();
        }
        let mut selected_completion = None;
        let mut neighbor_completion = None;
        for completion in schedule.poll().into_completions() {
            let parts = completion.into_parts();
            if parts.0 == selected {
                selected_completion = Some(parts);
            } else if parts.0 == neighbor {
                neighbor_completion = Some(parts);
            }
        }
        let (completed_path, result, foreground_proven) = selected_completion.unwrap();
        let (_, neighbor_result, neighbor_foreground_proven) = neighbor_completion.unwrap();

        assert_eq!(completed_path, selected);
        assert!(matches!(result, Err(PrefetchFailure::Decode)));
        assert!(matches!(neighbor_result, Ok(None)));
        assert!(selected_cancelled.load(Ordering::Acquire));
        assert!(!neighbor_cancelled.load(Ordering::Acquire));
        assert!(foreground_proven);
        assert!(!neighbor_foreground_proven);
        assert!(!schedule.record_outcome(&completed_path, true, foreground_proven));
        assert!(schedule.is_eligible(&completed_path));
    }

    #[test]
    fn terminal_failure_is_suppressed_until_foreground_success_or_reset() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("broken.png");
        assert!(schedule.request_with(
            path.clone(),
            || {},
            |task| {
                task();
                true
            },
            |_, _| Err(PrefetchFailure::Decode),
        ));
        let completion = schedule.poll().into_completions().pop().unwrap();
        let (completed_path, _, foreground_proven) = completion.into_parts();
        assert!(schedule.record_outcome(&completed_path, true, foreground_proven));
        assert!(!schedule.is_eligible(&path));

        schedule.allow(&path);
        assert!(schedule.is_eligible(&path));
        assert!(schedule.request_with(
            path.clone(),
            || {},
            |task| {
                task();
                true
            },
            |_, _| Err(PrefetchFailure::Decode),
        ));
        let completion = schedule.poll().into_completions().pop().unwrap();
        let (completed_path, _, foreground_proven) = completion.into_parts();
        assert!(schedule.record_outcome(&completed_path, true, foreground_proven));
        schedule.reset();
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn dropped_and_panicking_workers_have_stable_terminal_failures() {
        let mut dropped = PrefetchSchedule::default();
        assert!(dropped.request_with(
            PathBuf::from("dropped.png"),
            || {},
            |task| {
                drop(task);
                true
            },
            |_, _| Ok(None),
        ));
        let completion = dropped.poll().into_completions().pop().unwrap();
        let (_, result, _) = completion.into_parts();
        assert!(matches!(result, Err(PrefetchFailure::WorkerDisconnected)));

        let mut panicked = PrefetchSchedule::default();
        assert!(panicked.request_with(
            PathBuf::from("panicked.png"),
            || {},
            |task| {
                task();
                true
            },
            |_, _| panic!("worker panic"),
        ));
        let completion = panicked.poll().into_completions().pop().unwrap();
        let (_, result, _) = completion.into_parts();
        assert!(matches!(result, Err(PrefetchFailure::WorkerPanicked)));
    }

    #[test]
    fn privacy_safe_filename_is_bounded_control_safe_and_path_free() {
        let path = PathBuf::from("private-folder").join(format!(
            "bad\n\u{202e}hidden\u{2028}{} .png",
            "x".repeat(200)
        ));
        let name = privacy_safe_file_name(&path);

        assert!(!name.contains("private-folder"));
        assert!(!name.chars().any(char::is_control));
        assert!(!name.contains('\u{202e}'));
        assert!(!name.contains('\u{2028}'));
        assert!(name.starts_with("bad??hidden?"));
        assert!(name.chars().count() <= 96);
    }

    #[test]
    fn privacy_safe_filename_handles_non_unicode_names() {
        #[cfg(unix)]
        let path = {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(OsString::from_vec(vec![0xff]))
        };
        #[cfg(windows)]
        let path = {
            use std::ffi::OsString;
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(OsString::from_wide(&[0xd800]))
        };

        assert_eq!(privacy_safe_file_name(&path), "<non-Unicode filename>");
    }

    #[test]
    fn replacement_and_clear_keep_byte_accounting_exact() {
        let mut cache = PrefetchCache::with_limits(5, 20);
        cache.insert(PathBuf::from("a"), bytes(1, 4));
        cache.insert(PathBuf::from("a"), bytes(2, 7));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes(), 7);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
    }
}
