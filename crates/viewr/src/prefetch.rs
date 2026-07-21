//! Bounded in-memory neighbor decode cache (privacy-safe: never on disk).
//!
//! When the user arrows through a folder, next/previous images should already
//! be decoded in RAM. This is an LRU of [`DecodedImage`] keyed by path — no
//! thumbnail database, no history file, cleared when the process exits.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use crate::decode::DecodedImage;

/// Default number of decoded full images kept around the current one.
pub const DEFAULT_CAPACITY: usize = 5;

/// LRU cache of fully decoded images for instant navigation.
#[derive(Default)]
pub struct PrefetchCache {
    capacity: usize,
    order: VecDeque<PathBuf>,
    images: HashMap<PathBuf, DecodedImage>,
}

impl PrefetchCache {
    /// Create a cache that holds at most `capacity` images (minimum 1).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            images: HashMap::new(),
        }
    }

    /// Number of cached images.
    #[must_use]
    pub fn len(&self) -> usize {
        self.images.len()
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
        self.order.retain(|p| p != path);
        Some(img)
    }

    /// Insert or replace; evicts least-recently-used entries past capacity.
    pub fn insert(&mut self, path: PathBuf, image: DecodedImage) {
        if self.images.contains_key(&path) {
            self.order.retain(|p| p != &path);
        }
        self.order.push_back(path.clone());
        self.images.insert(path, image);
        while self.images.len() > self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.images.remove(&old);
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
    }
}
