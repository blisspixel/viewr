//! Bounded in-memory neighbor decode cache (privacy-safe: never on disk).
//!
//! When the user arrows through a folder, next/previous images should already
//! be decoded in RAM. This is an LRU of [`DecodedImage`] keyed by path, with no
//! thumbnail database, no history file, cleared when the process exits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::decode::DecodedImage;

/// Default number of decoded full images kept around the current one.
pub const DEFAULT_CAPACITY: usize = 5;
/// Default decoded-neighbor RAM budget: 256 MiB.
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_SAFE_FILENAME_CHARS: usize = 96;

/// Cancellation and result-generation identity for one speculative decode.
#[derive(Clone, Debug)]
pub struct PrefetchTicket {
    generation: u64,
    cancellation: Arc<AtomicU64>,
}

impl PrefetchTicket {
    /// Speculative schedule generation captured by this job.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Cancellation generation observed by the checked decoder boundary.
    #[must_use]
    pub fn cancellation_generation(&self) -> &AtomicU64 {
        &self.cancellation
    }

    /// Whether the speculative decode still owns useful work.
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.cancellation.load(Ordering::Acquire) == 0
    }
}

/// Per-playlist scheduling state for speculative neighbor decodes.
///
/// Paths with terminal outcomes remain suppressed until the playlist changes or
/// a successful foreground presentation proves that the path is usable again.
#[derive(Debug, Default)]
pub struct PrefetchSchedule {
    generation: u64,
    in_flight: HashMap<PathBuf, Arc<AtomicU64>>,
    terminal: HashSet<PathBuf>,
    foreground_proven: HashSet<PathBuf>,
}

impl PrefetchSchedule {
    /// Whether a path can start a speculative decode in the current generation.
    #[must_use]
    pub fn is_eligible(&self, path: &Path) -> bool {
        !self.in_flight.contains_key(path) && !self.terminal.contains(path)
    }

    /// Mark a path as scheduled and return the playlist generation for its job.
    pub fn start(&mut self, path: PathBuf) -> Option<PrefetchTicket> {
        if !self.is_eligible(&path) {
            return None;
        }
        let cancellation = Arc::new(AtomicU64::new(0));
        self.in_flight.insert(path, Arc::clone(&cancellation));
        Some(PrefetchTicket {
            generation: self.generation,
            cancellation,
        })
    }

    /// Whether a result belongs to a job still active in the current generation.
    #[must_use]
    pub fn accepts_result(&self, generation: u64, path: &Path) -> bool {
        generation == self.generation && self.in_flight.contains_key(path)
    }

    /// Whether a trusted presentation made this speculative result obsolete.
    ///
    /// The result still needs to be finished so its in-flight bookkeeping is
    /// removed, but decoded pixels from it must not replace the trusted image.
    #[must_use]
    pub fn result_was_superseded(&self, generation: u64, path: &Path) -> bool {
        generation == self.generation && self.foreground_proven.contains(path)
    }

    /// Finish a current-generation job.
    ///
    /// Returns `None` for stale or unknown work. Accepted work returns whether
    /// it created an effective terminal transition after foreground recovery.
    pub fn finish(&mut self, generation: u64, path: &Path, terminal: bool) -> Option<bool> {
        if generation != self.generation || self.in_flight.remove(path).is_none() {
            return None;
        }
        let foreground_proven = self.foreground_proven.remove(path);
        let effective_terminal = terminal && !foreground_proven;
        if effective_terminal {
            self.terminal.insert(path.to_owned());
        }
        Some(effective_terminal)
    }

    /// Start a new playlist generation and forget all prior scheduling state.
    pub fn reset(&mut self) {
        for cancellation in self.in_flight.values() {
            cancellation.store(1, Ordering::Release);
        }
        self.generation = self.generation.wrapping_add(1);
        self.in_flight.clear();
        self.terminal.clear();
        self.foreground_proven.clear();
    }

    /// Allow a path again after a successful trusted presentation.
    pub fn allow(&mut self, path: &Path) {
        self.terminal.remove(path);
        if let Some(cancellation) = self.in_flight.get(path) {
            cancellation.store(1, Ordering::Release);
            self.foreground_proven.insert(path.to_owned());
        }
    }

    /// Number of current-generation jobs still in flight.
    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    /// Whether no current-generation job is in flight.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_empty()
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
    use super::{PrefetchCache, PrefetchSchedule, neighbor_indices, privacy_safe_file_name};
    use crate::color::WorkingColorEncoding;
    use crate::decode::DecodedImage;
    use std::path::PathBuf;
    use std::sync::Arc;

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
    fn terminal_attempt_is_scheduled_once_per_playlist_generation() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("broken.png");
        assert!(schedule.is_idle());
        let ticket = schedule.start(path.clone()).expect("first attempt");
        let generation = ticket.generation();

        assert!(!schedule.is_eligible(&path));
        assert_eq!(schedule.in_flight_len(), 1);
        assert!(!schedule.is_idle());
        assert_eq!(schedule.finish(generation, &path, true), Some(true));
        assert!(schedule.is_idle());
        assert!(!schedule.is_eligible(&path));
        assert!(schedule.start(path.clone()).is_none());

        schedule.reset();
        assert!(schedule.is_eligible(&path));
        assert_ne!(schedule.start(path).unwrap().generation(), generation);
    }

    #[test]
    fn scheduler_rejection_remains_retryable() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("queued.png");
        let generation = schedule
            .start(path.clone())
            .expect("queue attempt")
            .generation();

        assert_eq!(schedule.finish(generation, &path, false), Some(false));
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn stale_completion_cannot_mutate_current_generation() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("shared.png");
        let stale_generation = schedule
            .start(path.clone())
            .expect("old attempt")
            .generation();
        schedule.reset();
        let current_generation = schedule
            .start(path.clone())
            .expect("current attempt")
            .generation();

        assert!(!schedule.accepts_result(stale_generation, &path));
        assert!(schedule.accepts_result(current_generation, &path));
        assert_eq!(schedule.finish(stale_generation, &path, true), None);
        assert!(!schedule.is_eligible(&path));
        assert_eq!(
            schedule.finish(current_generation, &path, false),
            Some(false)
        );
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn successful_foreground_presentation_reopens_terminal_path() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("repaired.png");
        let generation = schedule
            .start(path.clone())
            .expect("prefetch attempt")
            .generation();
        assert_eq!(schedule.finish(generation, &path, true), Some(true));

        schedule.allow(&path);
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn successful_foreground_presentation_overrides_late_prefetch_failure() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("foreground.png");
        let generation = schedule
            .start(path.clone())
            .expect("prefetch attempt")
            .generation();

        schedule.allow(&path);
        assert!(schedule.result_was_superseded(generation, &path));
        assert_eq!(schedule.finish(generation, &path, true), Some(false));
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn trusted_presentation_supersedes_late_prefetch_pixels() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("retained.png");
        let generation = schedule
            .start(path.clone())
            .expect("prefetch attempt")
            .generation();

        schedule.allow(&path);

        assert!(schedule.accepts_result(generation, &path));
        assert!(schedule.result_was_superseded(generation, &path));
        assert_eq!(schedule.finish(generation, &path, false), Some(false));
        assert!(!schedule.result_was_superseded(generation, &path));
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn explicit_reload_generation_rejects_prior_work() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("reloaded.png");
        let before_reload = schedule
            .start(path.clone())
            .expect("prefetch attempt")
            .generation();

        schedule.reset();
        assert!(!schedule.accepts_result(before_reload, &path));
        assert_eq!(schedule.finish(before_reload, &path, true), None);
        assert!(schedule.is_eligible(&path));
    }

    #[test]
    fn reset_cooperatively_cancels_underlying_speculative_work() {
        let mut schedule = PrefetchSchedule::default();
        let path = PathBuf::from("obsolete.png");
        let ticket = schedule.start(path).expect("prefetch attempt");
        assert!(ticket.is_current());

        schedule.reset();
        assert!(!ticket.is_current());
    }

    #[test]
    fn foreground_success_cancels_only_the_same_path_ticket() {
        let mut schedule = PrefetchSchedule::default();
        let selected = PathBuf::from("selected.png");
        let neighbor = PathBuf::from("neighbor.png");
        let selected_ticket = schedule.start(selected.clone()).unwrap();
        let neighbor_ticket = schedule.start(neighbor).unwrap();

        schedule.allow(&selected);

        assert!(!selected_ticket.is_current());
        assert!(neighbor_ticket.is_current());
        assert_eq!(
            schedule.finish(selected_ticket.generation(), &selected, true),
            Some(false)
        );
    }

    #[test]
    fn finish_reports_only_effective_terminal_transitions() {
        let mut schedule = PrefetchSchedule::default();
        let terminal = PathBuf::from("terminal.png");
        let recovered = PathBuf::from("recovered.png");
        let terminal_ticket = schedule.start(terminal.clone()).unwrap();
        let recovered_ticket = schedule.start(recovered.clone()).unwrap();
        schedule.allow(&recovered);

        assert_eq!(
            schedule.finish(terminal_ticket.generation(), &terminal, true),
            Some(true)
        );
        assert_eq!(
            schedule.finish(recovered_ticket.generation(), &recovered, true),
            Some(false)
        );
        assert_eq!(
            schedule.finish(recovered_ticket.generation(), &recovered, true),
            None
        );
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
