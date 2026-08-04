//! Playlist management and scanning logic.

use crate::fs::{ScanProvenance, ScannedImage};
use crate::ratings::{RatingFilter, RatingState};
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) struct Playlist {
    pub(crate) files: Vec<PathBuf>,
    provenance: Vec<Option<ScanProvenance>>,
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

/// True when applying the filter selection must clear or replace the current image.
#[must_use]
pub(crate) const fn filter_selection_changes_source(
    selection: FilterSelection,
    has_current_image: bool,
) -> bool {
    match selection {
        FilterSelection::Stay => !has_current_image,
        FilterSelection::Select(_) | FilterSelection::Empty => true,
    }
}

impl Playlist {
    pub(crate) fn new(files: Vec<PathBuf>, index: usize) -> Self {
        let provenance = vec![None; files.len()];
        Self::from_parts(files, provenance, index)
    }

    pub(crate) fn from_scan(entries: Vec<ScannedImage>, index: usize) -> Self {
        let (files, provenance) = entries
            .into_iter()
            .map(ScannedImage::into_parts)
            .map(|(path, provenance)| (path, Some(provenance)))
            .unzip();
        Self::from_parts(files, provenance, index)
    }

    fn from_parts(
        files: Vec<PathBuf>,
        provenance: Vec<Option<ScanProvenance>>,
        index: usize,
    ) -> Self {
        let index = index.min(files.len().saturating_sub(1));
        let ratings = vec![RatingState::Loading; files.len()];
        let visible_indices = (0..files.len()).collect();
        Self {
            files,
            provenance,
            index,
            ratings,
            filter: RatingFilter::All,
            visible_indices,
            empty_anchor: index,
            outside_filter: false,
        }
    }

    pub(crate) fn scan_provenance(&self, path: &std::path::Path) -> Option<ScanProvenance> {
        self.files
            .iter()
            .position(|candidate| candidate == path)
            .and_then(|index| self.provenance.get(index))
            .copied()
            .flatten()
    }

    pub(crate) fn files_with_provenance(
        &self,
    ) -> impl ExactSizeIterator<Item = (&std::path::Path, Option<ScanProvenance>)> + '_ {
        self.files
            .iter()
            .map(PathBuf::as_path)
            .zip(self.provenance.iter().copied())
    }

    pub(crate) fn set_scan_provenance(
        &mut self,
        path: &std::path::Path,
        provenance: Option<ScanProvenance>,
    ) -> bool {
        let Some(index) = self.files.iter().position(|candidate| candidate == path) else {
            return false;
        };
        self.provenance[index] = provenance;
        true
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
        let mut kept_provenance = Vec::with_capacity(self.provenance.len());
        for ((path, rating), provenance) in self
            .files
            .drain(..)
            .zip(self.ratings.drain(..))
            .zip(self.provenance.drain(..))
        {
            if !removed.contains(&path) {
                kept_files.push(path);
                kept_ratings.push(rating);
                kept_provenance.push(provenance);
            }
        }
        self.files = kept_files;
        self.ratings = kept_ratings;
        self.provenance = kept_provenance;
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

    pub(crate) fn insert_path(
        &mut self,
        index: usize,
        path: PathBuf,
        rating: RatingState,
        provenance: Option<ScanProvenance>,
    ) {
        let index = index.min(self.files.len());
        self.files.insert(index, path);
        self.ratings.insert(index, rating);
        self.provenance.insert(index, provenance);
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
    use crate::ephemeral::TempWorkspace;
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

    #[test]
    fn provenance_iteration_visits_a_maximum_size_playlist_once_in_order() {
        let files = (0..100_000)
            .map(|index| PathBuf::from(format!("image-{index}.jpg")))
            .collect::<Vec<_>>();
        let playlist = Playlist::new(files, 0);
        let mut expected = 0_usize;

        for (path, provenance) in playlist.files_with_provenance() {
            assert_eq!(path, PathBuf::from(format!("image-{expected}.jpg")));
            assert!(provenance.is_none());
            expected += 1;
        }

        assert_eq!(expected, 100_000);
    }

    #[test]
    fn scanned_playlist_preserves_provenance_and_clamps_its_selection() {
        let workspace = TempWorkspace::new("playlist_scan_provenance").unwrap();
        let first = workspace.path().join("image-2.jpg");
        let second = workspace.path().join("image-10.jpg");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let entries = crate::fs::scan_image_entries_while(workspace.path(), || true).unwrap();

        let mut playlist = Playlist::from_scan(entries, usize::MAX);

        assert_eq!(
            playlist
                .files
                .iter()
                .filter_map(|path| path.file_name())
                .collect::<Vec<_>>(),
            ["image-2.jpg", "image-10.jpg"]
        );
        assert_eq!(playlist.index, 1);
        assert_eq!(playlist.filter(), RatingFilter::All);
        assert!(playlist.has_loading_ratings());
        let scanned_first = playlist.files[0].clone();
        let scanned_second = playlist.files[1].clone();
        assert!(playlist.scan_provenance(&scanned_first).is_some());
        assert!(playlist.scan_provenance(&scanned_second).is_some());
        assert!(!playlist.set_scan_provenance(PathBuf::from("missing.jpg").as_path(), None));
        assert!(playlist.set_scan_provenance(&scanned_first, None));
        assert!(playlist.scan_provenance(&scanned_first).is_none());
        assert!(playlist.scan_provenance(&scanned_second).is_some());
    }

    #[test]
    fn empty_playlist_operations_are_total_and_non_mutating() {
        let mut playlist = Playlist::new(Vec::new(), usize::MAX);
        let threshold = RatingFilter::AtLeast(Rating::new(5).unwrap());

        assert_eq!(playlist.current_rating(), RatingState::Loading);
        assert!(!playlist.has_loading_ratings());
        assert_eq!(playlist.visible_position(), None);
        assert_eq!(playlist.set_filter(threshold), FilterSelection::Empty);
        assert_eq!(playlist.show_all(), FilterSelection::Empty);
        assert_eq!(playlist.navigation_target(1), None);
        assert_eq!(playlist.navigation_target(0), None);
        assert!(!playlist.dismiss_outside_filter());
        assert!(!playlist.select(0));
        assert!(!playlist.set_rating(PathBuf::from("missing.jpg").as_path(), RatingState::Unrated));
        assert!(playlist.visible_catalog_range().is_empty());
        assert!(playlist.visible_neighbor_paths(4).is_empty());
    }

    #[test]
    fn insertion_removal_and_filter_recovery_keep_parallel_state_aligned() {
        let mut playlist = rated_playlist(1);
        let threshold = RatingFilter::AtLeast(Rating::new(4).unwrap());
        assert_eq!(playlist.set_filter(threshold), FilterSelection::Select(2));
        playlist.index = 2;
        assert_eq!(playlist.show_all(), FilterSelection::Select(1));
        assert!(playlist.select(1));

        assert_eq!(playlist.set_filter(threshold), FilterSelection::Select(2));
        assert!(playlist.select(2));
        playlist.insert_path(
            0,
            path(99),
            RatingState::Rated(Rating::new(5).unwrap()),
            None,
        );
        assert_eq!(playlist.index, 3);
        assert_eq!(playlist.files[playlist.index], path(2));
        assert_eq!(
            playlist.current_rating(),
            RatingState::Rated(Rating::new(4).unwrap())
        );

        playlist.index = 4;
        playlist.remove_paths(&[path(0), path(1)], 4);
        assert_eq!(
            playlist.files,
            [path(99), path(2), path(3), path(4), path(5), path(6)]
        );
        assert_eq!(playlist.files[playlist.index], path(4));
        assert_eq!(
            playlist.current_rating(),
            RatingState::Rated(Rating::new(5).unwrap())
        );
        assert_eq!(playlist.provenance.len(), playlist.files.len());
        assert_eq!(playlist.ratings.len(), playlist.files.len());
    }
}
