//! Selection geometry over display coordinates (row = position in the sorted,
//! filtered view, not the original row number). No GPUI types.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

impl Cell {
    pub fn new(row: usize, col: usize) -> Self {
        Cell { row, col }
    }
}

/// An inclusive, normalised rectangle (`r0 <= r1`, `c0 <= c1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub r0: usize,
    pub r1: usize,
    pub c0: usize,
    pub c1: usize,
}

impl CellRange {
    pub fn from_corners(a: Cell, b: Cell) -> Self {
        CellRange { r0: a.row.min(b.row), r1: a.row.max(b.row), c0: a.col.min(b.col), c1: a.col.max(b.col) }
    }
    pub fn rows(&self) -> usize {
        self.r1 - self.r0 + 1
    }
    pub fn cols(&self) -> usize {
        self.c1 - self.c0 + 1
    }
    pub fn cell_count(&self) -> usize {
        self.rows() * self.cols()
    }
    pub fn contains(&self, c: Cell) -> bool {
        (self.r0..=self.r1).contains(&c.row) && (self.c0..=self.c1).contains(&c.col)
    }
    pub fn is_single(&self) -> bool {
        self.r0 == self.r1 && self.c0 == self.c1
    }
}

/// What a click selected; decides how the rectangle follows the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
    Cells,
    /// Whole rows: the column span is always every column.
    Rows,
    /// Whole columns: the row span is always every row.
    Cols,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cell,
    /// The "current cell": shown with the ring and the target of editing.
    pub cursor: Cell,
    pub extent: Extent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Up,
    Down,
    Left,
    Right,
    PageUp(usize),
    PageDown(usize),
    RowStart,
    RowEnd,
    Top,
    Bottom,
    /// Tab: next cell, wrapping to the next row.
    Next,
    /// Shift-Tab.
    Prev,
}

impl Selection {
    pub fn single(cell: Cell) -> Self {
        Selection { anchor: cell, cursor: cell, extent: Extent::Cells }
    }

    pub fn row(row: usize, col: usize) -> Self {
        Selection { anchor: Cell::new(row, col), cursor: Cell::new(row, col), extent: Extent::Rows }
    }

    pub fn column(row: usize, col: usize) -> Self {
        Selection { anchor: Cell::new(row, col), cursor: Cell::new(row, col), extent: Extent::Cols }
    }

    pub fn all(nrows: usize, ncols: usize) -> Self {
        Selection {
            anchor: Cell::new(0, 0),
            cursor: Cell::new(nrows.saturating_sub(1), ncols.saturating_sub(1)),
            extent: Extent::Cells,
        }
    }

    /// The selected rectangle, expanded to full rows/columns for those extents.
    pub fn range(&self, nrows: usize, ncols: usize) -> CellRange {
        let mut r = CellRange::from_corners(self.anchor, self.cursor);
        match self.extent {
            Extent::Cells => {}
            Extent::Rows => {
                r.c0 = 0;
                r.c1 = ncols.saturating_sub(1);
            }
            Extent::Cols => {
                r.r0 = 0;
                r.r1 = nrows.saturating_sub(1);
            }
        }
        r
    }

    /// Move the cursor by `motion`. Without `extend` the selection collapses
    /// to the new cell; with it the anchor stays and the rectangle grows.
    pub fn moved(&self, motion: Motion, extend: bool, nrows: usize, ncols: usize) -> Selection {
        let cursor = apply_motion(self.cursor, motion, nrows, ncols);
        if extend {
            Selection { cursor, ..*self }
        } else {
            Selection::single(cursor)
        }
    }

    /// Shift-click / drag: keep the anchor, move the cursor to `cell`.
    pub fn extended_to(&self, cell: Cell, nrows: usize, ncols: usize) -> Selection {
        Selection { cursor: clamp_cell(cell, nrows, ncols), ..*self }
    }

    /// After the data shrinks (filter, applied deletes) keep the selection valid.
    pub fn clamped(&self, nrows: usize, ncols: usize) -> Selection {
        Selection {
            anchor: clamp_cell(self.anchor, nrows, ncols),
            cursor: clamp_cell(self.cursor, nrows, ncols),
            extent: self.extent,
        }
    }
}

pub fn clamp_cell(c: Cell, nrows: usize, ncols: usize) -> Cell {
    Cell { row: c.row.min(nrows.saturating_sub(1)), col: c.col.min(ncols.saturating_sub(1)) }
}

pub fn apply_motion(c: Cell, motion: Motion, nrows: usize, ncols: usize) -> Cell {
    if nrows == 0 || ncols == 0 {
        return Cell::default();
    }
    let (last_r, last_c) = (nrows - 1, ncols - 1);
    let c = clamp_cell(c, nrows, ncols);
    match motion {
        Motion::Up => Cell::new(c.row.saturating_sub(1), c.col),
        Motion::Down => Cell::new((c.row + 1).min(last_r), c.col),
        Motion::Left => Cell::new(c.row, c.col.saturating_sub(1)),
        Motion::Right => Cell::new(c.row, (c.col + 1).min(last_c)),
        Motion::PageUp(n) => Cell::new(c.row.saturating_sub(n.max(1)), c.col),
        Motion::PageDown(n) => Cell::new((c.row + n.max(1)).min(last_r), c.col),
        Motion::RowStart => Cell::new(c.row, 0),
        Motion::RowEnd => Cell::new(c.row, last_c),
        Motion::Top => Cell::new(0, 0),
        Motion::Bottom => Cell::new(last_r, last_c),
        Motion::Next => {
            if c.col < last_c {
                Cell::new(c.row, c.col + 1)
            } else if c.row < last_r {
                Cell::new(c.row + 1, 0)
            } else {
                c
            }
        }
        Motion::Prev => {
            if c.col > 0 {
                Cell::new(c.row, c.col - 1)
            } else if c.row > 0 {
                Cell::new(c.row - 1, last_c)
            } else {
                c
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_is_normalised() {
        let r = CellRange::from_corners(Cell::new(5, 4), Cell::new(2, 1));
        assert_eq!((r.r0, r.r1, r.c0, r.c1), (2, 5, 1, 4));
        assert_eq!(r.cell_count(), 16);
        assert!(r.contains(Cell::new(3, 2)));
        assert!(!r.contains(Cell::new(6, 2)));
        assert!(CellRange::from_corners(Cell::new(1, 1), Cell::new(1, 1)).is_single());
    }

    #[test]
    fn rows_and_columns_expand_to_full_span() {
        let s = Selection::row(3, 2);
        let r = s.range(10, 5);
        assert_eq!((r.r0, r.r1, r.c0, r.c1), (3, 3, 0, 4));
        let s = Selection::column(3, 2);
        let r = s.range(10, 5);
        assert_eq!((r.r0, r.r1, r.c0, r.c1), (0, 9, 2, 2));
        let all = Selection::all(10, 5).range(10, 5);
        assert_eq!(all.cell_count(), 50);
    }

    #[test]
    fn extending_keeps_anchor() {
        let s = Selection::single(Cell::new(2, 2));
        let s = s.moved(Motion::Down, true, 10, 5).moved(Motion::Right, true, 10, 5);
        assert_eq!(s.anchor, Cell::new(2, 2));
        assert_eq!(s.range(10, 5), CellRange { r0: 2, r1: 3, c0: 2, c1: 3 });
        let s = s.moved(Motion::Up, true, 10, 5).moved(Motion::Up, true, 10, 5);
        assert_eq!(s.range(10, 5), CellRange { r0: 1, r1: 2, c0: 2, c1: 3 });
        // collapsing
        let s = s.moved(Motion::Left, false, 10, 5);
        assert_eq!(s, Selection::single(Cell::new(1, 2)));
    }

    #[test]
    fn row_selection_extends_by_rows() {
        let s = Selection::row(2, 0).extended_to(Cell::new(5, 3), 10, 5);
        assert_eq!(s.range(10, 5), CellRange { r0: 2, r1: 5, c0: 0, c1: 4 });
    }

    #[test]
    fn motions_clamp_at_edges() {
        let c = Cell::new(0, 0);
        assert_eq!(apply_motion(c, Motion::Up, 3, 3), c);
        assert_eq!(apply_motion(c, Motion::Left, 3, 3), c);
        assert_eq!(apply_motion(Cell::new(2, 2), Motion::Down, 3, 3), Cell::new(2, 2));
        assert_eq!(apply_motion(Cell::new(1, 1), Motion::PageDown(50), 10, 3), Cell::new(9, 1));
        assert_eq!(apply_motion(Cell::new(8, 1), Motion::PageUp(5), 10, 3), Cell::new(3, 1));
        assert_eq!(apply_motion(Cell::new(4, 1), Motion::RowStart, 10, 3), Cell::new(4, 0));
        assert_eq!(apply_motion(Cell::new(4, 1), Motion::RowEnd, 10, 3), Cell::new(4, 2));
        assert_eq!(apply_motion(Cell::new(4, 1), Motion::Top, 10, 3), Cell::new(0, 0));
        assert_eq!(apply_motion(Cell::new(4, 1), Motion::Bottom, 10, 3), Cell::new(9, 2));
    }

    #[test]
    fn tab_wraps_rows() {
        assert_eq!(apply_motion(Cell::new(0, 2), Motion::Next, 3, 3), Cell::new(1, 0));
        assert_eq!(apply_motion(Cell::new(2, 2), Motion::Next, 3, 3), Cell::new(2, 2));
        assert_eq!(apply_motion(Cell::new(1, 0), Motion::Prev, 3, 3), Cell::new(0, 2));
        assert_eq!(apply_motion(Cell::new(0, 0), Motion::Prev, 3, 3), Cell::new(0, 0));
    }

    #[test]
    fn empty_grid_is_safe() {
        assert_eq!(apply_motion(Cell::new(4, 4), Motion::Down, 0, 3), Cell::default());
        assert_eq!(Selection::all(0, 0).range(0, 0).cell_count(), 1);
    }

    #[test]
    fn clamping_after_shrink() {
        let s = Selection::single(Cell::new(50, 9)).clamped(10, 4);
        assert_eq!(s.cursor, Cell::new(9, 3));
    }
}
