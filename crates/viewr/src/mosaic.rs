//! Pure policy and geometry for the full-image mosaic.
//!
//! The mosaic is a transient view over the active playlist projection. It does
//! not create thumbnails, a catalog, or any durable state. The event loop owns
//! decoded-image admission and the GPU owns the corresponding full-image
//! textures.

use crate::view::PhysicalViewport;

/// Maximum number of complete photos shown together.
pub(crate) const MAX_IMAGES: usize = 8;

/// One page in the active playlist projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MosaicPage {
    /// Projection offset of the first image in this page.
    pub(crate) start: usize,
    /// Canonical catalog indices, in projection order.
    pub(crate) indices: Vec<usize>,
    /// Focused slot within `indices`.
    pub(crate) focused: usize,
}

impl MosaicPage {
    /// Build the page containing `current`, or the closest page when the current
    /// image is outside the active rating projection.
    #[must_use]
    pub(crate) fn containing(
        projection: &[usize],
        current: usize,
        page_size: usize,
    ) -> Option<Self> {
        if projection.is_empty() || page_size == 0 {
            return None;
        }
        let position = projection
            .binary_search(&current)
            .unwrap_or_else(|insertion| insertion.min(projection.len().saturating_sub(1)));
        let start = position / page_size * page_size;
        Self::at(projection, start, page_size, position)
    }

    /// Build a page at a projection offset and focus the closest requested
    /// projection position.
    #[must_use]
    pub(crate) fn at(
        projection: &[usize],
        requested_start: usize,
        page_size: usize,
        requested_focus: usize,
    ) -> Option<Self> {
        if projection.is_empty() || page_size == 0 {
            return None;
        }
        let last_start = projection.len().saturating_sub(1) / page_size * page_size;
        let start = requested_start.min(last_start) / page_size * page_size;
        let end = start.saturating_add(page_size).min(projection.len());
        let focused_position = requested_focus.clamp(start, end.saturating_sub(1));
        Some(Self {
            start,
            indices: projection[start..end].to_vec(),
            focused: focused_position.saturating_sub(start),
        })
    }

    /// Move to the adjacent page without wrapping.
    #[must_use]
    pub(crate) fn adjacent(&self, projection: &[usize], delta: isize) -> Option<Self> {
        let next_start = match delta.cmp(&0) {
            std::cmp::Ordering::Less => self.start.saturating_sub(MAX_IMAGES),
            std::cmp::Ordering::Greater => self.start.saturating_add(MAX_IMAGES),
            std::cmp::Ordering::Equal => self.start,
        };
        if next_start == self.start && delta != 0 {
            return None;
        }
        let next = Self::at(projection, next_start, MAX_IMAGES, next_start)?;
        (next.start != self.start).then_some(next)
    }

    /// Move keyboard focus through the adaptive grid without wrapping.
    pub(crate) fn move_focus(&mut self, direction: FocusDirection, columns: usize) {
        if self.indices.is_empty() {
            self.focused = 0;
            return;
        }
        let columns = columns.max(1);
        self.focused = match direction {
            FocusDirection::Previous => self.focused.saturating_sub(1),
            FocusDirection::Next => self
                .focused
                .saturating_add(1)
                .min(self.indices.len().saturating_sub(1)),
            FocusDirection::Up => self.focused.saturating_sub(columns),
            FocusDirection::Down => {
                let same_column = self.focused.saturating_add(columns);
                if same_column < self.indices.len() {
                    same_column
                } else {
                    self.indices.len().saturating_sub(1)
                }
            }
            FocusDirection::First => 0,
            FocusDirection::Last => self.indices.len().saturating_sub(1),
        };
    }
}

/// Keyboard focus movement within one mosaic page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusDirection {
    Previous,
    Next,
    Up,
    Down,
    First,
    Last,
}

/// Slots that own grid positions for the current frame.
///
/// Loading reserves every target position so completed photos do not jump after
/// each upload. A terminal partial page compacts only the photos that succeeded.
#[must_use]
pub(crate) fn layout_slots(loaded: &[usize], target: usize, loading: bool) -> Vec<usize> {
    if loading {
        (0..target.min(MAX_IMAGES)).collect()
    } else {
        loaded
            .iter()
            .copied()
            .filter(|slot| *slot < target.min(MAX_IMAGES))
            .collect()
    }
}

/// Adaptive grid geometry for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MosaicGrid {
    pub(crate) columns: usize,
    pub(crate) cells: Vec<PhysicalViewport>,
}

/// Lay out `count` photos inside `viewport`, centering an incomplete final row.
///
/// The chosen column count maximizes the smaller cell axis, which produces a
/// 4-by-2 grid on a landscape screen and a 2-by-4 grid on a portrait screen for
/// eight photos. Gaps are physical pixels and are reduced safely in tiny views.
#[must_use]
pub(crate) fn adaptive_grid(
    viewport: PhysicalViewport,
    count: usize,
    requested_gap: u32,
) -> MosaicGrid {
    let count = count.min(MAX_IMAGES);
    if count == 0 {
        return MosaicGrid {
            columns: 1,
            cells: Vec::new(),
        };
    }
    let columns = (1..=count)
        .min_by(|left, right| {
            grid_score(viewport, count, *right, requested_gap)
                .total_cmp(&grid_score(viewport, count, *left, requested_gap))
                .then_with(|| left.cmp(right))
        })
        .unwrap_or(1);
    let rows = count.div_ceil(columns);
    let gap = bounded_gap(viewport, columns, rows, requested_gap);
    let available_width = viewport
        .width
        .saturating_sub(gap.saturating_mul(columns.saturating_sub(1) as u32));
    let available_height = viewport
        .height
        .saturating_sub(gap.saturating_mul(rows.saturating_sub(1) as u32));
    let cell_width = (available_width / columns as u32).max(1);
    let cell_height = (available_height / rows as u32).max(1);
    let mut cells = Vec::with_capacity(count);
    for index in 0..count {
        let row = index / columns;
        let column = index % columns;
        let in_row = (count - row * columns).min(columns);
        let row_width = cell_width
            .saturating_mul(in_row as u32)
            .saturating_add(gap.saturating_mul(in_row.saturating_sub(1) as u32));
        let row_x = viewport
            .x
            .saturating_add(viewport.width.saturating_sub(row_width) / 2);
        cells.push(PhysicalViewport {
            x: row_x.saturating_add((cell_width + gap).saturating_mul(column as u32)),
            y: viewport
                .y
                .saturating_add((cell_height + gap).saturating_mul(row as u32)),
            width: cell_width,
            height: cell_height,
        });
    }
    MosaicGrid { columns, cells }
}

fn grid_score(viewport: PhysicalViewport, count: usize, columns: usize, requested_gap: u32) -> f64 {
    let rows = count.div_ceil(columns);
    let gap = bounded_gap(viewport, columns, rows, requested_gap);
    let width = viewport
        .width
        .saturating_sub(gap.saturating_mul(columns.saturating_sub(1) as u32))
        / columns as u32;
    let height = viewport
        .height
        .saturating_sub(gap.saturating_mul(rows.saturating_sub(1) as u32))
        / rows as u32;
    f64::from(width.min(height))
}

fn bounded_gap(viewport: PhysicalViewport, columns: usize, rows: usize, requested: u32) -> u32 {
    let horizontal = if columns > 1 {
        viewport.width.saturating_sub(columns as u32) / (columns - 1) as u32
    } else {
        requested
    };
    let vertical = if rows > 1 {
        viewport.height.saturating_sub(rows as u32) / (rows - 1) as u32
    } else {
        requested
    };
    requested.min(horizontal).min(vertical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> Vec<usize> {
        vec![0, 2, 4, 6, 8, 10, 12, 14, 16, 18]
    }

    #[test]
    fn page_contains_current_and_respects_filtered_projection() {
        let first = MosaicPage::containing(&projection(), 6, MAX_IMAGES).unwrap();
        assert_eq!(first.start, 0);
        assert_eq!(first.indices, [0, 2, 4, 6, 8, 10, 12, 14]);
        assert_eq!(first.focused, 3);

        let last = MosaicPage::containing(&projection(), 18, MAX_IMAGES).unwrap();
        assert_eq!(last.start, 8);
        assert_eq!(last.indices, [16, 18]);
        assert_eq!(last.focused, 1);
    }

    #[test]
    fn current_outside_filter_uses_the_closest_projected_photo() {
        let page = MosaicPage::containing(&projection(), 7, MAX_IMAGES).unwrap();
        assert_eq!(page.focused, 4);
        assert_eq!(page.indices[page.focused], 8);
        assert!(MosaicPage::containing(&[], 0, MAX_IMAGES).is_none());
        assert!(MosaicPage::containing(&projection(), 0, 0).is_none());
    }

    #[test]
    fn page_navigation_is_bounded_and_focus_moves_in_the_grid() {
        let first = MosaicPage::containing(&projection(), 0, MAX_IMAGES).unwrap();
        assert!(first.adjacent(&projection(), -1).is_none());
        let mut last = first.adjacent(&projection(), 1).unwrap();
        assert_eq!(last.indices, [16, 18]);
        assert!(last.adjacent(&projection(), 1).is_none());
        last.move_focus(FocusDirection::Last, 2);
        assert_eq!(last.focused, 1);
        last.move_focus(FocusDirection::Up, 2);
        assert_eq!(last.focused, 0);
        last.move_focus(FocusDirection::Down, 2);
        assert_eq!(last.focused, 1);
        last.move_focus(FocusDirection::First, 2);
        last.move_focus(FocusDirection::Previous, 2);
        assert_eq!(last.focused, 0);
        last.move_focus(FocusDirection::Next, 2);
        assert_eq!(last.focused, 1);
    }

    #[test]
    fn eight_photos_use_landscape_and_portrait_grids() {
        let landscape = adaptive_grid(
            PhysicalViewport {
                x: 0,
                y: 0,
                width: 1600,
                height: 900,
            },
            8,
            8,
        );
        assert_eq!(landscape.columns, 4);
        assert_eq!(landscape.cells.len(), 8);

        let portrait = adaptive_grid(
            PhysicalViewport {
                x: 0,
                y: 0,
                width: 900,
                height: 1600,
            },
            8,
            8,
        );
        assert_eq!(portrait.columns, 2);
        assert_eq!(portrait.cells.len(), 8);
    }

    #[test]
    fn incomplete_last_row_is_centered_and_tiny_views_stay_valid() {
        let grid = adaptive_grid(
            PhysicalViewport {
                x: 10,
                y: 20,
                width: 1000,
                height: 600,
            },
            5,
            10,
        );
        assert_eq!(grid.columns, 3);
        assert!(grid.cells[3].x > grid.cells[0].x);
        assert!(
            grid.cells
                .iter()
                .all(|cell| cell.width > 0 && cell.height > 0)
        );

        let tiny = adaptive_grid(
            PhysicalViewport {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            8,
            20,
        );
        assert_eq!(tiny.cells.len(), 8);
        assert!(
            tiny.cells
                .iter()
                .all(|cell| cell.width > 0 && cell.height > 0)
        );
        assert!(
            adaptive_grid(
                PhysicalViewport {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10
                },
                0,
                8
            )
            .cells
            .is_empty()
        );
    }

    #[test]
    fn progressive_loading_reserves_stable_slots_then_compacts_once() {
        assert_eq!(layout_slots(&[0], 8, true), (0..8).collect::<Vec<_>>());
        assert_eq!(
            layout_slots(&[0, 2, 5], 8, true),
            (0..8).collect::<Vec<_>>()
        );
        assert_eq!(layout_slots(&[0, 2, 5], 8, false), [0, 2, 5]);
        assert_eq!(layout_slots(&[0, 9], 9, false), [0]);
    }
}
