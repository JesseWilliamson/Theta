//! Column widths and scroll-window arithmetic. Pure `f32` maths so the
//! virtualisation rules can be tested without a window.

use std::ops::Range;

pub const MIN_COL_W: f32 = 60.0;
pub const MAX_COL_W: f32 = 400.0;
/// Horizontal cell padding on each side.
pub const CELL_PAD: f32 = 8.0;
/// Rows sampled when sizing columns; scanning all 100k would stall opening a result.
pub const SAMPLE_ROWS: usize = 200;

/// Width that fits the header and the widest sampled cell, clamped.
/// `header_px` is the already-measured header content (name + type + icons).
pub fn auto_width(header_px: f32, max_cell_chars: usize, char_w: f32) -> f32 {
    let cell_px = max_cell_chars as f32 * char_w + CELL_PAD * 2.0 + 2.0;
    header_px.max(cell_px).clamp(MIN_COL_W, MAX_COL_W)
}

/// Gutter wide enough for the largest row number (plus the pending "new" rows).
pub fn gutter_width(max_row_number: usize, char_w: f32) -> f32 {
    let digits = max_row_number.max(1).to_string().len().max(2);
    (digits as f32 * char_w + 20.0).ceil()
}

#[derive(Debug, Clone, Default)]
pub struct ColumnLayout {
    widths: Vec<f32>,
    /// `offsets[i]` is the left edge of column `i`; `offsets[n]` the total width.
    offsets: Vec<f32>,
}

impl ColumnLayout {
    pub fn new(widths: Vec<f32>) -> Self {
        let mut l = ColumnLayout { widths, offsets: Vec::new() };
        l.rebuild();
        l
    }

    fn rebuild(&mut self) {
        self.offsets.clear();
        self.offsets.reserve(self.widths.len() + 1);
        let mut x = 0.0;
        self.offsets.push(x);
        for w in &self.widths {
            x += w;
            self.offsets.push(x);
        }
    }

    pub fn len(&self) -> usize {
        self.widths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.widths.is_empty()
    }

    pub fn width(&self, col: usize) -> f32 {
        self.widths.get(col).copied().unwrap_or(MIN_COL_W)
    }

    pub fn left(&self, col: usize) -> f32 {
        self.offsets.get(col).copied().unwrap_or_else(|| self.total())
    }

    pub fn total(&self) -> f32 {
        self.offsets.last().copied().unwrap_or(0.0)
    }

    pub fn set_width(&mut self, col: usize, w: f32) {
        if let Some(slot) = self.widths.get_mut(col) {
            *slot = w.clamp(MIN_COL_W, 2000.0);
            self.rebuild();
        }
    }

    /// Columns intersecting `[scroll_x, scroll_x + viewport_w)`, widened by
    /// one column each side so half-visible neighbours are never missing
    /// during a fast scroll.
    pub fn visible(&self, scroll_x: f32, viewport_w: f32) -> Range<usize> {
        let n = self.widths.len();
        if n == 0 {
            return 0..0;
        }
        let first = self.column_at(scroll_x.max(0.0)).unwrap_or(n - 1);
        let last = self.column_at((scroll_x + viewport_w).max(0.0)).unwrap_or(n - 1);
        first.saturating_sub(1)..(last + 2).min(n)
    }

    /// Column containing content-space x, `None` past the right edge.
    pub fn column_at(&self, x: f32) -> Option<usize> {
        if x < 0.0 || x >= self.total() {
            return None;
        }
        // partition_point: number of offsets <= x, minus the leading 0.
        Some(self.offsets.partition_point(|&o| o <= x).saturating_sub(1).min(self.widths.len() - 1))
    }
}

/// Rows `[first, last]` visible plus `overscan` either side, clamped to `nrows`.
pub fn visible_rows(scroll_y: f32, viewport_h: f32, row_h: f32, nrows: usize, overscan: usize) -> Range<usize> {
    if nrows == 0 || viewport_h <= 0.0 {
        return 0..0;
    }
    let first = (scroll_y.max(0.0) / row_h).floor() as usize;
    let last = ((scroll_y.max(0.0) + viewport_h) / row_h).ceil() as usize;
    first.saturating_sub(overscan).min(nrows)..(last + overscan).min(nrows)
}

/// Scroll offset (content px, positive) that brings `[start, end)` into a
/// viewport of `size` currently scrolled to `scroll`, moving as little as possible.
pub fn scroll_to_reveal(scroll: f32, size: f32, start: f32, end: f32) -> f32 {
    if start < scroll {
        start
    } else if end > scroll + size {
        (end - size).max(0.0)
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_width_is_clamped() {
        assert_eq!(auto_width(30.0, 1, 7.0), MIN_COL_W);
        assert_eq!(auto_width(30.0, 1000, 7.0), MAX_COL_W);
        // header wider than the data wins
        assert_eq!(auto_width(150.0, 3, 7.0), 150.0);
        // data wider than the header wins: 10 chars * 7 + 16 + 2
        assert_eq!(auto_width(50.0, 10, 7.0), 88.0);
    }

    #[test]
    fn gutter_fits_digits() {
        assert_eq!(gutter_width(5, 7.0), 34.0); // 2 digits minimum
        assert_eq!(gutter_width(100_000, 7.0), 62.0);
        assert!(gutter_width(1_000_000, 7.0) > gutter_width(100_000, 7.0));
    }

    #[test]
    fn column_lookup() {
        let l = ColumnLayout::new(vec![100.0, 50.0, 200.0]);
        assert_eq!(l.total(), 350.0);
        assert_eq!(l.left(2), 150.0);
        assert_eq!(l.column_at(0.0), Some(0));
        assert_eq!(l.column_at(99.9), Some(0));
        assert_eq!(l.column_at(100.0), Some(1));
        assert_eq!(l.column_at(349.0), Some(2));
        assert_eq!(l.column_at(350.0), None);
        assert_eq!(l.column_at(-1.0), None);
    }

    #[test]
    fn visible_columns_include_neighbours() {
        let l = ColumnLayout::new(vec![100.0; 10]);
        assert_eq!(l.visible(0.0, 250.0), 0..4);
        assert_eq!(l.visible(450.0, 250.0), 3..9);
        assert_eq!(l.visible(10_000.0, 250.0), 8..10);
        assert_eq!(ColumnLayout::default().visible(0.0, 100.0), 0..0);
    }

    #[test]
    fn resizing_rebuilds_offsets_and_clamps() {
        let mut l = ColumnLayout::new(vec![100.0, 100.0]);
        l.set_width(0, 10.0);
        assert_eq!(l.width(0), MIN_COL_W);
        assert_eq!(l.left(1), MIN_COL_W);
        l.set_width(1, 300.0);
        assert_eq!(l.total(), MIN_COL_W + 300.0);
    }

    #[test]
    fn visible_row_window() {
        assert_eq!(visible_rows(0.0, 240.0, 24.0, 1000, 2), 0..12);
        assert_eq!(visible_rows(2400.0, 240.0, 24.0, 1000, 2), 98..112);
        assert_eq!(visible_rows(0.0, 240.0, 24.0, 5, 2), 0..5);
        assert_eq!(visible_rows(0.0, 240.0, 24.0, 0, 2), 0..0);
        // far past the end clamps, never panics
        assert_eq!(visible_rows(1e9, 240.0, 24.0, 10, 2), 10..10);
    }

    #[test]
    fn reveal_moves_minimally() {
        assert_eq!(scroll_to_reveal(100.0, 200.0, 150.0, 180.0), 100.0);
        assert_eq!(scroll_to_reveal(100.0, 200.0, 50.0, 80.0), 50.0);
        assert_eq!(scroll_to_reveal(100.0, 200.0, 290.0, 320.0), 120.0);
    }
}
