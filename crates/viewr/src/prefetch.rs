//! Bounded in-memory neighbor decode cache (privacy-safe: never on disk).
//!
//! When the user arrows through a folder, next/previous images should already
//! be decoded in RAM. This is an LRU of [`DecodedImage`] keyed by path, with no
//! thumbnail database, no history file, cleared when the process exits.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::decode::DecodedImage;

/// Default number of decoded full images kept around the current one.
pub const DEFAULT_CAPACITY: usize = 5;
/// Default decoded-neighbor RAM budget: 256 MiB.
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

/// LRU cache of fully decoded images for instant navigation.
pub struct PrefetchCache {
    capacity: usize,
    max_bytes: usize,
    current_bytes: usize,
    order: VecDeque<PathBuf>,
    images: HashMap<PathBuf, DecodedImage>,
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
        self.images.get(path)
    }

    /// Take ownership of a cached image (removes it from the cache).
    pub fn take(&mut self, path: &Path) -> Option<DecodedImage> {
        let img = self.images.remove(path)?;
        self.current_bytes = self.current_bytes.saturating_sub(img.rgba.len());
        self.order.retain(|p| p != path);
        Some(img)
    }

    /// Insert or replace, evicting least-recently-used entries until both
    /// limits hold. An image larger than the complete byte budget is not cached.
    pub fn insert(&mut self, path: PathBuf, image: DecodedImage) {
        if let Some(replaced) = self.images.remove(&path) {
            self.current_bytes = self.current_bytes.saturating_sub(replaced.rgba.len());
            self.order.retain(|p| p != &path);
        }
        let image_bytes = image.rgba.len();
        if image_bytes > self.max_bytes {
            return;
        }
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
    use super::{PrefetchCache, neighbor_indices};
    use crate::decode::DecodedImage;
    use std::path::PathBuf;

    fn tiny(id: u8) -> DecodedImage {
        DecodedImage {
            rgba: vec![id, 0, 0, 255],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
        }
    }

    fn bytes(id: u8, len: usize) -> DecodedImage {
        DecodedImage {
            rgba: vec![id; len],
            width: 1,
            height: 1,
            color_profile: crate::decode::ColorProfileStatus::AssumedSrgb,
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
        cache.insert(PathBuf::from("large"), bytes(1, 5));
        assert!(cache.is_empty());
        assert_eq!(cache.bytes(), 0);
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
