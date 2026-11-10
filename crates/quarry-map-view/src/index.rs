//! A uniform grid over feature bounding boxes.
//!
//! Used both to cull what is drawn (rectangle query) and to find hover candidates
//! (tiny rectangle around the pointer), so neither is O(all features) in the common
//! case of a zoomed-in viewport on a big result.

use quarry_geo::BBox;

/// Maximum cells per side.
const MAX_DIM: usize = 256;
/// A box overlapping more cells than this goes to the always-checked `big` list instead of
/// being copied into hundreds of buckets.
const BIG_CELLS: usize = 64;

/// Bucketed bounding boxes.
#[derive(Clone, Debug)]
pub struct GridIndex {
    origin: [f64; 2],
    cell: [f64; 2],
    dim: [usize; 2],
    cells: Vec<Vec<u32>>,
    big: Vec<u32>,
    boxes: Vec<BBox>,
}

impl GridIndex {
    /// Index `boxes`; ids returned by queries are positions in this slice.
    pub fn new(boxes: &[BBox]) -> GridIndex {
        let total = boxes.iter().copied().reduce(BBox::union);
        let Some(total) = total else {
            return GridIndex {
                origin: [0.0; 2],
                cell: [1.0; 2],
                dim: [1, 1],
                cells: vec![vec![]],
                big: vec![],
                boxes: vec![],
            };
        };
        // About two features per cell on average, square-ish.
        let side = ((boxes.len() as f64 / 2.0).sqrt().ceil() as usize).clamp(1, MAX_DIM);
        let dim = [side, side];
        let cell = [
            (total.width() / side as f64).max(f64::MIN_POSITIVE * 1e10),
            (total.height() / side as f64).max(f64::MIN_POSITIVE * 1e10),
        ];
        let mut idx = GridIndex {
            origin: total.min,
            cell,
            dim,
            cells: vec![Vec::new(); side * side],
            big: Vec::new(),
            boxes: boxes.to_vec(),
        };
        for (i, b) in boxes.iter().enumerate() {
            let ((x0, y0), (x1, y1)) = idx.cell_range(b);
            if (x1 - x0 + 1) * (y1 - y0 + 1) > BIG_CELLS {
                idx.big.push(i as u32);
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    idx.cells[y * side + x].push(i as u32);
                }
            }
        }
        idx
    }

    fn cell_of(&self, v: f64, axis: usize) -> usize {
        let f = ((v - self.origin[axis]) / self.cell[axis]).floor();
        if f.is_nan() { 0 } else { f.clamp(0.0, (self.dim[axis] - 1) as f64) as usize }
    }

    fn cell_range(&self, b: &BBox) -> ((usize, usize), (usize, usize)) {
        (
            (self.cell_of(b.min[0], 0), self.cell_of(b.min[1], 1)),
            (self.cell_of(b.max[0], 0), self.cell_of(b.max[1], 1)),
        )
    }

    /// True when nothing is indexed.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// Ids of all boxes intersecting `rect`, ascending and without duplicates.
    #[cfg(test)]
    pub fn query(&self, rect: &BBox) -> Vec<u32> {
        let mut out = Vec::new();
        self.query_into(rect, &mut out);
        out
    }

    /// Like [`GridIndex::query`] but reusing `out` (cleared first).
    pub fn query_into(&self, rect: &BBox, out: &mut Vec<u32>) {
        out.clear();
        if self.boxes.is_empty() {
            return;
        }
        let ((x0, y0), (x1, y1)) = self.cell_range(rect);
        let cells = (x1 - x0 + 1) * (y1 - y0 + 1);
        // When the query covers most of the grid, walking every box once beats
        // gathering and de-duplicating bucket contents.
        if cells * 2 >= self.dim[0] * self.dim[1] {
            out.extend(
                self.boxes.iter().enumerate().filter(|(_, b)| b.intersects(rect)).map(|(i, _)| i as u32),
            );
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend(
                    self.cells[y * self.dim[0] + x]
                        .iter()
                        .copied()
                        .filter(|i| self.boxes[*i as usize].intersects(rect)),
                );
            }
        }
        out.extend(self.big.iter().copied().filter(|i| self.boxes[*i as usize].intersects(rect)));
        out.sort_unstable();
        out.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x0: f64, y0: f64, x1: f64, y1: f64) -> BBox {
        BBox { min: [x0, y0], max: [x1, y1] }
    }

    fn brute(boxes: &[BBox], r: &BBox) -> Vec<u32> {
        boxes.iter().enumerate().filter(|(_, x)| x.intersects(r)).map(|(i, _)| i as u32).collect()
    }

    #[test]
    fn matches_brute_force_on_random_boxes() {
        let mut s = 12345u64;
        let mut rnd = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 33) as f64 / (1u64 << 31) as f64
        };
        let boxes: Vec<BBox> = (0..2000)
            .map(|i| {
                let (x, y) = (rnd(), rnd());
                let w = if i % 97 == 0 { 0.6 } else { rnd() * 0.02 };
                b(x, y, x + w, y + w * 0.5)
            })
            .collect();
        let idx = GridIndex::new(&boxes);
        for _ in 0..200 {
            let (x, y) = (rnd(), rnd());
            let r = b(x, y, x + rnd() * 0.3, y + rnd() * 0.3);
            assert_eq!(idx.query(&r), brute(&boxes, &r));
        }
    }

    #[test]
    fn point_features_and_degenerate_extent() {
        let boxes = vec![b(1.0, 1.0, 1.0, 1.0), b(1.0, 1.0, 1.0, 1.0)];
        let idx = GridIndex::new(&boxes);
        assert_eq!(idx.query(&b(0.9, 0.9, 1.1, 1.1)), vec![0, 1]);
        assert!(idx.query(&b(2.0, 2.0, 3.0, 3.0)).is_empty());
    }

    #[test]
    fn empty_index() {
        let idx = GridIndex::new(&[]);
        assert!(idx.is_empty());
        assert!(idx.query(&b(0.0, 0.0, 1.0, 1.0)).is_empty());
    }

    #[test]
    fn big_boxes_are_found_from_any_cell() {
        let mut boxes: Vec<BBox> = (0..400).map(|i| b(i as f64, 0.0, i as f64 + 0.5, 0.5)).collect();
        boxes.push(b(0.0, 0.0, 400.0, 0.5));
        let idx = GridIndex::new(&boxes);
        let hit = idx.query(&b(250.1, 0.1, 250.2, 0.2));
        assert!(hit.contains(&400) && hit.contains(&250));
    }
}
