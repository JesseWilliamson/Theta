//! Per-zoom-level simplification cache.
//!
//! Simplifying a 100k-vertex polygon to what a screen can show is O(n log n); doing it per
//! frame would stall panning. Results are cached by `(integer level, feature)` so a zoom
//! gesture computes each level once and panning at a level is free.

use quarry_geo::{Geometry, simplify};
use std::collections::HashMap;
use std::sync::Arc;

/// Geometries with fewer vertices than this are drawn as they are.
pub const MIN_VERTICES: usize = 32;
/// Target maximum on-screen error of simplification, in pixels.
pub const TOLERANCE_PX: f64 = 0.6;
/// Cached vertices above which the cache is flushed.
const BUDGET_VERTICES: usize = 4_000_000;

/// Cache of simplified geometries.
#[derive(Default)]
pub struct LodCache {
    map: HashMap<(i32, usize), Arc<Geometry>>,
    vertices: usize,
}

impl LodCache {
    /// Forget everything (new data set).
    pub fn clear(&mut self) {
        self.map.clear();
        self.vertices = 0;
    }

    /// Entries currently cached.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// True when nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The cached simplification for `(level, feature)` if one exists.
    pub fn peek(&self, level: i32, feature: usize) -> Option<&Arc<Geometry>> {
        self.map.get(&(level, feature))
    }

    /// The simplification of `geom` for `level`, computing and caching it on a miss.
    /// `None` means "use the original" (small geometries, or nothing was removed).
    pub fn get(
        &mut self,
        level: i32,
        feature: usize,
        geom: &Geometry,
        vertices: usize,
    ) -> Option<Arc<Geometry>> {
        if vertices < MIN_VERTICES {
            return None;
        }
        if let Some(g) = self.map.get(&(level, feature)) {
            return Some(g.clone());
        }
        if matches!(geom, Geometry::Point(_) | Geometry::MultiPoint(_)) {
            return None;
        }
        // `level` is floor(log2(px per world unit)), so one world unit is at least
        // 2^level px and TOLERANCE_PX px is at most TOLERANCE_PX / 2^level units.
        let tol = TOLERANCE_PX / 2f64.powi(level);
        let simple = Arc::new(simplify(geom, tol));
        let n = simple.coord_count();
        if self.vertices + n > BUDGET_VERTICES {
            self.clear();
        }
        self.vertices += n;
        self.map.insert((level, feature), simple.clone());
        Some(simple)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wiggly(n: usize) -> Geometry {
        Geometry::LineString((0..n).map(|i| [i as f64 * 1e-6, if i % 2 == 0 { 0.0 } else { 1e-9 }]).collect())
    }

    #[test]
    fn small_geometries_are_not_simplified() {
        let mut c = LodCache::default();
        let g = wiggly(10);
        assert!(c.get(10, 0, &g, 10).is_none());
        assert!(c.is_empty());
    }

    #[test]
    fn simplifies_and_caches_by_level() {
        let mut c = LodCache::default();
        let g = wiggly(1000);
        // At level 10 one world unit is >= 1024 px, so a 1e-9 wiggle is far below a pixel.
        let s = c.get(10, 7, &g, 1000).unwrap();
        assert!(s.coord_count() < 20, "{}", s.coord_count());
        assert_eq!(c.len(), 1);
        let again = c.get(10, 7, &g, 1000).unwrap();
        assert!(Arc::ptr_eq(&s, &again), "second call must hit the cache");
        // A different level is a different entry.
        c.get(20, 7, &g, 1000).unwrap();
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn points_are_never_simplified() {
        let mut c = LodCache::default();
        let g = Geometry::MultiPoint((0..100).map(|i| [i as f64, 0.0]).collect());
        assert!(c.get(3, 0, &g, 100).is_none());
    }
}
