//! Pure policy and geometry for the full-image mosaic.
//!
//! The mosaic is a transient view over the active playlist projection. It does
//! not create thumbnails, a catalog, or any durable state. The event loop owns
//! decoded-image admission and the GPU owns the corresponding full-image
//! textures.

use crate::view::PhysicalViewport;

/// Maximum number of complete photos admitted to one collage group.
pub(crate) const MAX_IMAGES: usize = 24;

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

    /// Move keyboard focus through the collage order without wrapping.
    pub(crate) fn move_focus(&mut self, direction: FocusDirection) {
        if self.indices.is_empty() {
            self.focused = 0;
            return;
        }
        self.focused = match direction {
            FocusDirection::Previous => self.focused.saturating_sub(1),
            FocusDirection::Next => self
                .focused
                .saturating_add(1)
                .min(self.indices.len().saturating_sub(1)),
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
    First,
    Last,
}

/// Dense, aspect-ratio-preserving collage geometry for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MosaicGrid {
    pub(crate) rows: usize,
    pub(crate) cells: Vec<PhysicalViewport>,
}

/// Pack complete photos into justified rows using their actual aspect ratios.
///
/// Every settled row fills the available width when its resulting height fits
/// the viewport. Row breaks minimize both unused vertical space and abrupt row
/// height changes. A pathological group that cannot fit even as one row is
/// uniformly reduced and centered instead of cropping or distorting a photo.
#[must_use]
pub(crate) fn dense_collage(
    viewport: PhysicalViewport,
    image_sizes: &[(u32, u32)],
    requested_gap: u32,
) -> MosaicGrid {
    let pixel_capacity = usize::try_from(u64::from(viewport.width) * u64::from(viewport.height))
        .unwrap_or(usize::MAX);
    let count = image_sizes.len().min(MAX_IMAGES).min(pixel_capacity);
    if count == 0 {
        return MosaicGrid {
            rows: 0,
            cells: Vec::new(),
        };
    }
    let gap = bounded_gap(viewport, count, requested_gap);
    let aspects = image_sizes[..count]
        .iter()
        .map(|&(width, height)| {
            if width == 0 || height == 0 {
                1.0
            } else {
                f64::from(width) / f64::from(height)
            }
        })
        .collect::<Vec<_>>();
    let row_ends = best_row_ends(viewport, &aspects, gap);
    let rows = row_ends.len();
    let available_image_height = viewport
        .height
        .saturating_sub(gap.saturating_mul(rows.saturating_sub(1) as u32));
    let mut start = 0;
    let natural_heights = row_ends
        .iter()
        .map(|&end| {
            let height = justified_row_height(viewport.width, &aspects[start..end], gap);
            start = end;
            height
        })
        .collect::<Vec<_>>();
    let natural_total = natural_heights.iter().sum::<f64>();
    let scale = if natural_total > f64::from(available_image_height) && natural_total > 0.0 {
        f64::from(available_image_height) / natural_total
    } else {
        1.0
    };
    let quantized_image_height =
        rounded_positive_u32(natural_total * scale, available_image_height.max(1));
    let row_heights = proportional_lengths(&natural_heights, quantized_image_height);
    let collage_height = row_heights
        .iter()
        .copied()
        .sum::<u32>()
        .saturating_add(gap.saturating_mul(rows.saturating_sub(1) as u32));
    let mut y = viewport
        .y
        .saturating_add(viewport.height.saturating_sub(collage_height) / 2);
    let mut cells = Vec::with_capacity(count);
    start = 0;
    for ((end, natural_height), height) in
        row_ends.into_iter().zip(natural_heights).zip(row_heights)
    {
        let row_aspects = &aspects[start..end];
        let natural_width = viewport
            .width
            .saturating_sub(gap.saturating_mul(row_aspects.len().saturating_sub(1) as u32));
        let width_scale = if natural_height > 0.0 {
            f64::from(height) / natural_height
        } else {
            1.0
        };
        let image_width =
            rounded_positive_u32(f64::from(natural_width) * width_scale, natural_width);
        let widths = proportional_lengths(row_aspects, image_width);
        let row_width = widths
            .iter()
            .copied()
            .sum::<u32>()
            .saturating_add(gap.saturating_mul(widths.len().saturating_sub(1) as u32));
        let mut x = viewport
            .x
            .saturating_add(viewport.width.saturating_sub(row_width) / 2);
        for width in widths {
            cells.push(PhysicalViewport {
                x,
                y,
                width,
                height,
            });
            x = x.saturating_add(width).saturating_add(gap);
        }
        y = y.saturating_add(height).saturating_add(gap);
        start = end;
    }
    MosaicGrid { rows, cells }
}

fn best_row_ends(viewport: PhysicalViewport, aspects: &[f64], gap: u32) -> Vec<usize> {
    let mut best = None::<(f64, Vec<usize>)>;
    let max_columns = usize::try_from(viewport.width).unwrap_or(usize::MAX).max(1);
    let min_rows = aspects.len().div_ceil(max_columns);
    let max_rows = aspects.len().min(
        usize::try_from(viewport.height)
            .unwrap_or(usize::MAX)
            .max(1),
    );
    for rows in min_rows..=max_rows {
        let available_height = viewport
            .height
            .saturating_sub(gap.saturating_mul(rows.saturating_sub(1) as u32));
        let target_height = f64::from(available_height.max(1)) / rows as f64;
        let Some((variance, ends)) =
            partition_rows(aspects, rows, viewport.width, gap, target_height)
        else {
            continue;
        };
        let mut start = 0;
        let total_height = ends
            .iter()
            .map(|&end| {
                let height = justified_row_height(viewport.width, &aspects[start..end], gap);
                start = end;
                height
            })
            .sum::<f64>()
            + f64::from(gap.saturating_mul(rows.saturating_sub(1) as u32));
        let fill_error =
            (total_height - f64::from(viewport.height)) / f64::from(viewport.height.max(1));
        let overflow_weight = if fill_error > 0.0 { 4.0 } else { 2.0 };
        let score = variance / rows as f64 + fill_error * fill_error * overflow_weight;
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score < *best_score)
        {
            best = Some((score, ends));
        }
    }
    best.map_or_else(|| vec![aspects.len()], |(_, ends)| ends)
}

fn partition_rows(
    aspects: &[f64],
    rows: usize,
    width: u32,
    gap: u32,
    target_height: f64,
) -> Option<(f64, Vec<usize>)> {
    let count = aspects.len();
    let mut cost = vec![vec![f64::INFINITY; count + 1]; rows + 1];
    let mut previous = vec![vec![usize::MAX; count + 1]; rows + 1];
    cost[0][0] = 0.0;
    for row in 1..=rows {
        for end in row..=count.saturating_sub(rows - row) {
            for start in (row - 1)..end {
                if !cost[row - 1][start].is_finite() {
                    continue;
                }
                if end - start > usize::try_from(width).unwrap_or(usize::MAX) {
                    continue;
                }
                let height = justified_row_height(width, &aspects[start..end], gap);
                let delta = (height - target_height) / target_height.max(1.0);
                let candidate = cost[row - 1][start] + delta * delta;
                if candidate < cost[row][end] {
                    cost[row][end] = candidate;
                    previous[row][end] = start;
                }
            }
        }
    }
    if !cost[rows][count].is_finite() {
        return None;
    }
    let mut ends = vec![0; rows];
    let mut end = count;
    for row in (1..=rows).rev() {
        ends[row - 1] = end;
        end = previous[row][end];
    }
    Some((cost[rows][count], ends))
}

fn justified_row_height(width: u32, aspects: &[f64], gap: u32) -> f64 {
    let available = width
        .saturating_sub(gap.saturating_mul(aspects.len().saturating_sub(1) as u32))
        .max(1);
    f64::from(available) / aspects.iter().sum::<f64>().max(f64::EPSILON)
}

fn proportional_lengths(weights: &[f64], total: u32) -> Vec<u32> {
    if weights.is_empty() {
        return Vec::new();
    }
    if total < weights.len() as u32 {
        return vec![1; weights.len()];
    }
    let mut remaining_total = total;
    let mut remaining_weight = weights.iter().sum::<f64>();
    let mut lengths = Vec::with_capacity(weights.len());
    for (index, weight) in weights.iter().enumerate() {
        let remaining_items = weights.len() - index;
        let length = if remaining_items == 1 {
            remaining_total
        } else {
            let reserved = remaining_items.saturating_sub(1) as u32;
            rounded_positive_u32(
                f64::from(remaining_total) * *weight / remaining_weight.max(f64::EPSILON),
                remaining_total.saturating_sub(reserved),
            )
        };
        lengths.push(length);
        remaining_total = remaining_total.saturating_sub(length);
        remaining_weight -= *weight;
    }
    lengths
}

fn rounded_positive_u32(value: f64, upper: u32) -> u32 {
    // Geometry values are finite, positive, and explicitly bounded before this
    // final physical-pixel quantization.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        value.round().clamp(1.0, f64::from(upper.max(1))) as u32
    }
}

fn bounded_gap(viewport: PhysicalViewport, count: usize, requested: u32) -> u32 {
    if count <= 1 {
        return requested.min(viewport.width).min(viewport.height);
    }
    let separators = count.saturating_sub(1) as u32;
    requested
        .min(viewport.width.saturating_sub(count as u32) / separators.max(1))
        .min(viewport.height.saturating_sub(count as u32) / separators.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projection() -> Vec<usize> {
        (0..30).map(|index| index * 2).collect()
    }

    #[test]
    fn page_contains_current_and_respects_filtered_projection() {
        let first = MosaicPage::containing(&projection(), 6, MAX_IMAGES).unwrap();
        assert_eq!(first.start, 0);
        assert_eq!(first.indices.len(), MAX_IMAGES);
        assert_eq!(first.indices.first(), Some(&0));
        assert_eq!(first.indices.last(), Some(&46));
        assert_eq!(first.focused, 3);

        let last = MosaicPage::containing(&projection(), 58, MAX_IMAGES).unwrap();
        assert_eq!(last.start, 24);
        assert_eq!(last.indices, [48, 50, 52, 54, 56, 58]);
        assert_eq!(last.focused, 5);
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
    fn page_navigation_is_bounded_and_focus_follows_collage_order() {
        let first = MosaicPage::containing(&projection(), 0, MAX_IMAGES).unwrap();
        assert!(first.adjacent(&projection(), -1).is_none());
        let mut last = first.adjacent(&projection(), 1).unwrap();
        assert_eq!(last.indices, [48, 50, 52, 54, 56, 58]);
        assert!(last.adjacent(&projection(), 1).is_none());
        last.move_focus(FocusDirection::Last);
        assert_eq!(last.focused, 5);
        last.move_focus(FocusDirection::First);
        last.move_focus(FocusDirection::Previous);
        assert_eq!(last.focused, 0);
        last.move_focus(FocusDirection::Next);
        assert_eq!(last.focused, 1);
    }

    #[test]
    fn twelve_landscape_photos_fill_the_screen_in_justified_rows() {
        let sizes = vec![(4, 3); 12];
        let collage = dense_collage(
            PhysicalViewport {
                x: 0,
                y: 0,
                width: 1600,
                height: 900,
            },
            &sizes,
            6,
        );
        assert_eq!(collage.rows, 3);
        assert_eq!(collage.cells.len(), 12);
        let top = collage.cells.iter().map(|cell| cell.y).min().unwrap();
        let bottom = collage
            .cells
            .iter()
            .map(|cell| cell.y + cell.height)
            .max()
            .unwrap();
        assert!(top <= 8, "unexpected top margin: {top}");
        assert!(bottom >= 892, "unexpected bottom margin: {bottom}");
    }

    #[test]
    fn source_aspects_define_tiles_without_equal_cell_letterboxing() {
        let sizes = [(3, 4), (16, 9), (1, 1), (4, 3), (9, 16), (2, 1)];
        let collage = dense_collage(
            PhysicalViewport {
                x: 10,
                y: 20,
                width: 1400,
                height: 800,
            },
            &sizes,
            5,
        );
        assert_eq!(collage.cells.len(), sizes.len());
        for (cell, &(width, height)) in collage.cells.iter().zip(&sizes) {
            let actual = f64::from(cell.width) / f64::from(cell.height);
            let expected = f64::from(width) / f64::from(height);
            assert!(
                (actual - expected).abs() < 0.02,
                "{width}:{height} photo received {actual:.3} tile"
            );
        }
        assert_ne!(collage.cells[0].width, collage.cells[1].width);
        assert_ne!(collage.cells[0].height, collage.cells[4].height);
    }

    #[test]
    fn collage_accepts_twenty_four_photos_and_tiny_views_stay_safe() {
        let sizes = vec![(3, 4); MAX_IMAGES + 4];
        let full = dense_collage(
            PhysicalViewport {
                x: 0,
                y: 0,
                width: 1200,
                height: 800,
            },
            &sizes,
            4,
        );
        assert_eq!(full.cells.len(), MAX_IMAGES);
        assert!(full.rows > 1);

        let tiny = dense_collage(
            PhysicalViewport {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            &sizes[..8],
            20,
        );
        assert_eq!(tiny.cells.len(), 4);
        assert!(tiny.cells.iter().all(|cell| cell.width > 0
            && cell.height > 0
            && cell.x + cell.width <= 2
            && cell.y + cell.height <= 2));
        for (index, cell) in tiny.cells.iter().enumerate() {
            for other in &tiny.cells[index + 1..] {
                assert!(
                    cell.x + cell.width <= other.x
                        || other.x + other.width <= cell.x
                        || cell.y + cell.height <= other.y
                        || other.y + other.height <= cell.y
                );
            }
        }
        assert!(
            dense_collage(
                PhysicalViewport {
                    x: 0,
                    y: 0,
                    width: 10,
                    height: 10
                },
                &[],
                8
            )
            .cells
            .is_empty()
        );
    }

    #[test]
    fn varied_collages_stay_ordered_bounded_and_aspect_correct() {
        let sizes = [
            (3, 4),
            (16, 9),
            (1, 1),
            (4, 3),
            (9, 16),
            (2, 1),
            (5, 4),
            (4, 5),
        ]
        .into_iter()
        .cycle()
        .take(MAX_IMAGES)
        .collect::<Vec<_>>();
        for viewport in [
            PhysicalViewport {
                x: 13,
                y: 17,
                width: 1600,
                height: 900,
            },
            PhysicalViewport {
                x: 7,
                y: 11,
                width: 900,
                height: 1600,
            },
            PhysicalViewport {
                x: 3,
                y: 5,
                width: 640,
                height: 480,
            },
        ] {
            for count in 1..=MAX_IMAGES {
                let collage = dense_collage(viewport, &sizes[..count], 4);
                assert_eq!(collage.cells.len(), count);
                for (cell, &(width, height)) in collage.cells.iter().zip(&sizes) {
                    assert!(cell.x >= viewport.x && cell.y >= viewport.y);
                    assert!(cell.x + cell.width <= viewport.x + viewport.width);
                    assert!(cell.y + cell.height <= viewport.y + viewport.height);
                    let expected_width =
                        f64::from(cell.height) * f64::from(width) / f64::from(height);
                    assert!((f64::from(cell.width) - expected_width).abs() <= 3.0);
                }
                for (index, cell) in collage.cells.iter().enumerate() {
                    for other in &collage.cells[index + 1..] {
                        assert!(
                            cell.x + cell.width <= other.x
                                || other.x + other.width <= cell.x
                                || cell.y + cell.height <= other.y
                                || other.y + other.height <= cell.y
                        );
                    }
                }
            }
        }
    }
}
