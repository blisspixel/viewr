//! Playlist management and scanning logic.

use crate::ratings::{RatingFilter, RatingState};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) struct Playlist {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) index: usize,
    ratings: Vec<RatingState>,
    filter: RatingFilter,
    visible_indices: Vec<usize>,
    empty_anchor: usize,
    outside_filter: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FilterSelection {
    Stay,
    Select(usize),
    Empty,
}

impl Playlist {
    pub(crate) fn new(files: Vec<PathBuf>, index: usize) -> Self {
        let index = index.min(files.len().saturating_sub(1));
        let ratings = vec![RatingState::Loading; files.len()];
        let visible_indices = (0..files.len()).collect();
        Self {
            files,
            index,
            ratings,
            filter: RatingFilter::All,
            visible_indices,
            empty_anchor: index,
            outside_filter: false,
        }
    }

    pub(crate) const fn filter(&self) -> RatingFilter {
        self.filter
    }

    pub(crate) fn current_rating(&self) -> RatingState {
        self.ratings
            .get(self.index)
            .copied()
            .unwrap_or(RatingState::Loading)
    }

    pub(crate) fn rating_for_path(&self, path: &std::path::Path) -> RatingState {
        self.rating_for_known_path(path)
            .unwrap_or(RatingState::Loading)
    }

    pub(crate) fn rating_for_known_path(&self, path: &std::path::Path) -> Option<RatingState> {
        self.files
            .iter()
            .position(|candidate| candidate == path)
            .and_then(|index| self.ratings.get(index))
            .copied()
    }

    pub(crate) const fn outside_filter(&self) -> bool {
        self.outside_filter
    }

    pub(crate) fn visible_len(&self) -> usize {
        self.visible_indices.len()
    }

    pub(crate) fn has_loading_ratings(&self) -> bool {
        self.ratings.contains(&RatingState::Loading)
    }

    pub(crate) fn visible_position(&self) -> Option<usize> {
        self.visible_position_for_catalog_index(self.index)
    }

    pub(crate) fn visible_position_for_catalog_index(&self, index: usize) -> Option<usize> {
        self.visible_indices.binary_search(&index).ok()
    }

    pub(crate) fn set_filter(&mut self, filter: RatingFilter) -> FilterSelection {
        let previous_index = self.index;
        self.empty_anchor = previous_index;
        self.filter = filter;
        self.rebuild_visible();
        self.outside_filter = false;
        if self.visible_indices.is_empty() {
            return FilterSelection::Empty;
        }
        if self.visible_indices.contains(&previous_index) {
            return FilterSelection::Stay;
        }
        let selected = self
            .visible_indices
            .iter()
            .copied()
            .find(|index| *index > previous_index)
            .or_else(|| {
                self.visible_indices
                    .iter()
                    .rev()
                    .copied()
                    .find(|index| *index < previous_index)
            })
            .unwrap_or(self.visible_indices[0]);
        FilterSelection::Select(selected)
    }

    pub(crate) fn show_all(&mut self) -> FilterSelection {
        let anchor = self.empty_anchor.min(self.files.len().saturating_sub(1));
        self.filter = RatingFilter::All;
        self.rebuild_visible();
        self.outside_filter = false;
        if self.files.is_empty() {
            FilterSelection::Empty
        } else if self.index == anchor {
            FilterSelection::Stay
        } else {
            FilterSelection::Select(anchor)
        }
    }

    pub(crate) fn set_rating(&mut self, path: &std::path::Path, state: RatingState) -> bool {
        let Some(index) = self.files.iter().position(|candidate| candidate == path) else {
            return false;
        };
        self.ratings[index] = state;
        self.rebuild_visible();
        self.outside_filter = !state.matches(self.filter) && self.index == index;
        true
    }

    pub(crate) fn set_discovered_ratings(&mut self, ratings: &[(PathBuf, RatingState)]) {
        let discovered = ratings
            .iter()
            .map(|(path, state)| (path.as_path(), *state))
            .collect::<HashMap<_, _>>();
        for (path, slot) in self.files.iter().zip(&mut self.ratings) {
            if *slot == RatingState::Loading
                && let Some(state) = discovered.get(path.as_path())
            {
                *slot = *state;
            }
        }
        self.rebuild_visible();
        self.outside_filter = !self.current_rating().matches(self.filter);
    }

    pub(crate) fn navigation_target(&self, delta: isize) -> Option<usize> {
        if self.visible_indices.is_empty() || delta == 0 {
            return None;
        }
        if let Some(position) = self.visible_position() {
            let maximum = self.visible_indices.len().saturating_sub(1).cast_signed();
            let target = (position.cast_signed() + delta).clamp(0, maximum);
            return self.visible_indices.get(target.cast_unsigned()).copied();
        }
        if delta < -1 {
            return self.visible_indices.first().copied();
        }
        if delta > 1 {
            return self.visible_indices.last().copied();
        }
        if delta > 0 {
            self.visible_indices
                .iter()
                .copied()
                .find(|index| *index > self.index)
                .or_else(|| self.visible_indices.first().copied())
        } else {
            self.visible_indices
                .iter()
                .rev()
                .copied()
                .find(|index| *index < self.index)
                .or_else(|| self.visible_indices.last().copied())
        }
    }

    pub(crate) fn dismiss_outside_filter(&mut self) -> bool {
        let dismissed = self.outside_filter;
        self.outside_filter = false;
        dismissed
    }

    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.files.len() {
            return false;
        }
        self.index = index;
        self.empty_anchor = index;
        self.outside_filter = !self.current_rating().matches(self.filter);
        true
    }

    pub(crate) fn visible_catalog_range(&self) -> Vec<usize> {
        if self.visible_indices.is_empty() {
            return Vec::new();
        }
        let center = self.visible_position().unwrap_or_else(|| {
            self.visible_indices
                .partition_point(|index| *index < self.index)
                .min(self.visible_indices.len().saturating_sub(1))
        });
        let range =
            center.saturating_sub(4)..center.saturating_add(5).min(self.visible_indices.len());
        range
            .map(|position| self.visible_indices[position])
            .collect()
    }

    pub(crate) fn visible_neighbor_paths(&self, radius: usize) -> Vec<PathBuf> {
        if let Some(position) = self.visible_position() {
            return crate::prefetch::neighbor_indices(position, self.visible_indices.len(), radius)
                .into_iter()
                .map(|visible| self.files[self.visible_indices[visible]].clone())
                .collect();
        }
        let insertion = self
            .visible_indices
            .partition_point(|index| *index < self.index);
        let start = insertion.saturating_sub(radius);
        let end = insertion
            .saturating_add(radius)
            .min(self.visible_indices.len());
        self.visible_indices[start..end]
            .iter()
            .map(|index| self.files[*index].clone())
            .collect()
    }

    pub(crate) fn remove_paths(&mut self, removed: &[PathBuf], old_index: usize) {
        let mut kept_files = Vec::with_capacity(self.files.len());
        let mut kept_ratings = Vec::with_capacity(self.ratings.len());
        for (path, rating) in self.files.drain(..).zip(self.ratings.drain(..)) {
            if !removed.contains(&path) {
                kept_files.push(path);
                kept_ratings.push(rating);
            }
        }
        self.files = kept_files;
        self.ratings = kept_ratings;
        self.index = crate::curate::index_after_removals(&self.files, old_index, removed);
        self.empty_anchor = self.index;
        self.rebuild_visible();
        if !self.visible_indices.is_empty() && !self.visible_indices.contains(&self.index) {
            self.index = self
                .visible_indices
                .iter()
                .copied()
                .find(|index| *index >= self.index)
                .or_else(|| self.visible_indices.last().copied())
                .unwrap_or(self.index);
        }
        self.outside_filter = false;
    }

    pub(crate) fn insert_path(&mut self, index: usize, path: PathBuf, rating: RatingState) {
        let index = index.min(self.files.len());
        self.files.insert(index, path);
        self.ratings.insert(index, rating);
        if index <= self.index && self.files.len() > 1 {
            self.index = self.index.saturating_add(1);
        }
        self.rebuild_visible();
    }

    fn rebuild_visible(&mut self) {
        self.visible_indices.clear();
        self.visible_indices.extend(
            self.ratings
                .iter()
                .enumerate()
                .filter_map(|(index, state)| state.matches(self.filter).then_some(index)),
        );
    }
}

pub(crate) enum ScanPurpose {
    SelectedFile(PathBuf),
    OpenFolder,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratings::Rating;

    fn path(index: usize) -> PathBuf {
        PathBuf::from(format!("{index}.jpg"))
    }

    fn rated_playlist(current: usize) -> Playlist {
        let mut playlist = Playlist::new((0..7).map(path).collect(), current);
        let states = [
            RatingState::Unrated,
            RatingState::Rated(Rating::new(1).unwrap()),
            RatingState::Rated(Rating::new(4).unwrap()),
            RatingState::Rejected,
            RatingState::Rated(Rating::new(5).unwrap()),
            RatingState::Conflict,
            RatingState::Rated(Rating::new(3).unwrap()),
        ];
        for (index, state) in states.into_iter().enumerate() {
            playlist.set_rating(&path(index), state);
        }
        playlist
    }

    #[test]
    fn threshold_projection_matches_only_numeric_ratings() {
        let expectations = [
            (1, vec![1, 2, 4, 6]),
            (2, vec![2, 4, 6]),
            (3, vec![2, 4, 6]),
            (4, vec![2, 4]),
            (5, vec![4]),
        ];
        for (threshold, expected) in expectations {
            let mut playlist = rated_playlist(0);
            playlist.set_filter(RatingFilter::AtLeast(Rating::new(threshold).unwrap()));
            assert_eq!(playlist.visible_indices, expected);
        }
    }

    #[test]
    fn filter_selection_keeps_current_then_chooses_next_then_previous() {
        let mut matching = rated_playlist(2);
        assert_eq!(
            matching.set_filter(RatingFilter::AtLeast(Rating::new(4).unwrap())),
            FilterSelection::Stay
        );
        assert_eq!(matching.index, 2);

        let mut next = rated_playlist(1);
        assert_eq!(
            next.set_filter(RatingFilter::AtLeast(Rating::new(4).unwrap())),
            FilterSelection::Select(2)
        );
        assert_eq!(next.index, 1);

        let mut previous = rated_playlist(6);
        assert_eq!(
            previous.set_filter(RatingFilter::AtLeast(Rating::new(4).unwrap())),
            FilterSelection::Select(4)
        );
        assert_eq!(previous.index, 6);
    }

    #[test]
    fn empty_filter_retains_anchor_for_show_all() {
        let mut playlist = rated_playlist(3);
        assert_eq!(
            playlist.set_filter(RatingFilter::AtLeast(Rating::new(5).unwrap())),
            FilterSelection::Select(4)
        );
        playlist.set_rating(&path(4), RatingState::Unrated);
        assert_eq!(playlist.visible_len(), 0);
        assert_eq!(playlist.show_all(), FilterSelection::Stay);
        assert_eq!(playlist.index, 3);
    }

    #[test]
    fn lowering_current_rating_keeps_it_as_explicit_outside_filter() {
        let mut playlist = rated_playlist(2);
        playlist.set_filter(RatingFilter::AtLeast(Rating::new(4).unwrap()));
        assert!(playlist.set_rating(&path(2), RatingState::Rated(Rating::new(3).unwrap())));
        assert_eq!(playlist.index, 2);
        assert!(playlist.outside_filter());
        assert_eq!(playlist.navigation_target(1), Some(4));
        assert_eq!(playlist.navigation_target(-1), Some(4));
        assert_eq!(playlist.visible_neighbor_paths(2), [path(4)]);

        let mut after_last_match = rated_playlist(4);
        after_last_match.set_filter(RatingFilter::AtLeast(Rating::new(4).unwrap()));
        after_last_match.set_rating(&path(4), RatingState::Unrated);
        assert_eq!(after_last_match.navigation_target(1), Some(2));
        assert_eq!(after_last_match.navigation_target(-1), Some(2));

        let mut between_matches = rated_playlist(3);
        between_matches.set_filter(RatingFilter::AtLeast(Rating::new(3).unwrap()));
        assert_eq!(between_matches.navigation_target(-999_999), Some(2));
        assert_eq!(between_matches.navigation_target(999_999), Some(6));
    }

    #[test]
    fn discovery_maps_by_path_across_order_removal_and_preserves_newer_states() {
        let discovered = vec![
            (path(0), RatingState::Unrated),
            (path(1), RatingState::Rated(Rating::new(2).unwrap())),
            (path(2), RatingState::Rejected),
        ];
        let out_of_order = vec![
            discovered[1].clone(),
            discovered[0].clone(),
            discovered[2].clone(),
        ];

        let mut reordered = Playlist::new((0..3).map(path).collect(), 1);
        reordered.set_discovered_ratings(&out_of_order);
        assert_eq!(reordered.ratings[0], RatingState::Unrated);
        assert_eq!(
            reordered.ratings[1],
            RatingState::Rated(Rating::new(2).unwrap())
        );
        assert_eq!(reordered.ratings[2], RatingState::Rejected);

        let mut newer = Playlist::new((0..3).map(path).collect(), 1);
        newer.set_rating(&path(1), RatingState::Rated(Rating::new(5).unwrap()));
        newer.set_discovered_ratings(&discovered);
        assert_eq!(
            newer.ratings[1],
            RatingState::Rated(Rating::new(5).unwrap())
        );

        let mut removed = Playlist::new((0..3).map(path).collect(), 1);
        removed.remove_paths(&[path(0)], 1);
        removed.set_discovered_ratings(&discovered);
        assert_eq!(removed.files, [path(1), path(2)]);
        assert_eq!(
            removed.ratings[0],
            RatingState::Rated(Rating::new(2).unwrap())
        );
        assert_eq!(removed.ratings[1], RatingState::Rejected);
    }

    #[test]
    fn outside_only_match_can_transition_to_the_no_match_state() {
        let mut playlist = rated_playlist(4);
        playlist.set_filter(RatingFilter::AtLeast(Rating::new(5).unwrap()));
        playlist.set_rating(&path(4), RatingState::Unrated);
        assert!(playlist.outside_filter());
        assert_eq!(playlist.visible_len(), 0);
        assert!(playlist.dismiss_outside_filter());
        assert!(!playlist.outside_filter());
    }

    #[test]
    fn filtered_neighbors_and_filmstrip_exclude_hidden_entries() {
        let mut playlist = rated_playlist(2);
        playlist.set_filter(RatingFilter::AtLeast(Rating::new(3).unwrap()));
        assert_eq!(playlist.visible_indices, [2, 4, 6]);
        assert_eq!(playlist.visible_neighbor_paths(2), [path(4), path(6)]);
        assert_eq!(playlist.visible_catalog_range(), [2, 4, 6]);
        assert_eq!(playlist.visible_position_for_catalog_index(2), Some(0));
        assert_eq!(playlist.visible_position_for_catalog_index(4), Some(1));
        assert_eq!(playlist.visible_position_for_catalog_index(6), Some(2));
        assert_eq!(playlist.visible_position_for_catalog_index(1), None);
    }

    #[test]
    fn presented_path_rating_is_independent_of_selected_index() {
        let playlist = rated_playlist(4);
        assert_eq!(
            playlist.rating_for_path(&path(2)),
            RatingState::Rated(Rating::new(4).unwrap())
        );
        assert_eq!(
            playlist.current_rating(),
            RatingState::Rated(Rating::new(5).unwrap())
        );
        assert_eq!(
            playlist.rating_for_path(std::path::Path::new("missing.jpg")),
            RatingState::Loading
        );
        assert_eq!(
            playlist.rating_for_known_path(std::path::Path::new("missing.jpg")),
            None
        );
    }
}
