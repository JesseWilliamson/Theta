//! Pure tile bookkeeping: cache keys, LRU, disk layout, freshness, request planning and
//! the parent-tile fallback. No I/O and no GPUI so it is all unit-testable.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use theta_geo::TileId;

/// How long a cached tile file is trusted before it is re-fetched.
pub const FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
/// Decoded tiles kept in memory.
pub const MEMORY_TILES: usize = 256;
/// Concurrent network requests.
pub const MAX_IN_FLIGHT: usize = 4;

/// A minimal least-recently-used map.
#[derive(Debug)]
pub struct Lru<K: std::hash::Hash + Eq + Clone, V> {
    cap: usize,
    map: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: std::hash::Hash + Eq + Clone, V> Lru<K, V> {
    /// An empty cache holding at most `cap` entries (at least one).
    pub fn new(cap: usize) -> Self {
        Lru { cap: cap.max(1), map: HashMap::new(), order: VecDeque::new() }
    }

    fn touch(&mut self, k: &K) {
        if let Some(pos) = self.order.iter().position(|x| x == k) {
            self.order.remove(pos);
        }
        self.order.push_back(k.clone());
    }

    /// Insert or replace; evicts the least recently used entry when full.
    pub fn insert(&mut self, k: K, v: V) {
        self.map.insert(k.clone(), v);
        self.touch(&k);
        while self.map.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    /// Look up and mark as most recently used.
    pub fn get(&mut self, k: &K) -> Option<&V> {
        if self.map.contains_key(k) {
            self.touch(k);
        }
        self.map.get(k)
    }

    /// Look up without changing recency (usable while painting several tiles).
    pub fn peek(&self, k: &K) -> Option<&V> {
        self.map.get(k)
    }

    /// Whether `k` is present.
    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    /// Number of entries.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// The host part of a URL template, used as the cache directory name.
pub fn host_of(template: &str) -> String {
    let rest = template.split_once("://").map_or(template, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let clean: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':' { c } else { '_' })
        .map(|c| if c == ':' { '_' } else { c })
        .collect();
    if clean.is_empty() { "unknown".into() } else { clean }
}

/// The cache directory of one basemap: the host plus a hash of the whole template, so two
/// basemaps on one host (CARTO's light and dark styles, say) never share tiles.
pub fn cache_dir_name(template: &str) -> String {
    // FNV-1a: unlike `DefaultHasher` it is specified, so the directory (and the cached tiles
    // in it) survives a compiler upgrade.
    let hash = template.bytes().fold(0x811c_9dc5_u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    format!("{}-{hash:08x}", host_of(template))
}

/// `<root>/theta/tiles`, where root is `THETA_CACHE_DIR`, else the OS cache dir.
pub fn cache_root() -> PathBuf {
    let base = std::env::var_os("THETA_CACHE_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir);
    base.join("theta").join("tiles")
}

/// `<root>/<host>/<z>/<x>/<y>.png`. The extension is nominal: the bytes are whatever
/// the server sent and are sniffed on decode.
pub fn disk_path(root: &Path, template: &str, t: TileId) -> PathBuf {
    root.join(cache_dir_name(template))
        .join(t.z.to_string())
        .join(t.x.to_string())
        .join(format!("{}.png", t.y))
}

/// Whether a file modified at `modified` is still fresh at `now`. A modification time
/// in the future (clock change) counts as fresh rather than looping on re-fetches.
pub fn is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    match now.duration_since(modified) {
        Ok(age) => age <= FRESH_FOR,
        Err(_) => true,
    }
}

/// The ordered list of tiles to fetch: wanted order (nearest the centre first), minus
/// tiles already decoded, in flight, or failed; at most `limit` of them.
pub fn plan_requests(
    wanted: &[TileId],
    have: impl Fn(TileId) -> bool,
    in_flight: &HashSet<TileId>,
    failed: &HashSet<TileId>,
    limit: usize,
) -> Vec<TileId> {
    wanted
        .iter()
        .copied()
        .filter(|t| !have(*t) && !in_flight.contains(t) && !failed.contains(t))
        .take(limit)
        .collect()
}

/// For a tile that is not loaded yet, the region of an ancestor tile (up to `max_up`
/// levels up) that covers it: `(ancestor, [u0, v0, u1, v1])` with the rectangle as
/// fractions of the ancestor image.
pub fn ancestor_region(t: TileId, up: u8) -> Option<(TileId, [f32; 4])> {
    if up == 0 || up > t.z {
        return None;
    }
    let size = 1u64 << up;
    let (px, py) = (t.x as u64 >> up, t.y as u64 >> up);
    let (ox, oy) = (t.x as u64 & (size - 1), t.y as u64 & (size - 1));
    let s = size as f32;
    Some((
        TileId { z: t.z - up, x: px as u32, y: py as u32 },
        [ox as f32 / s, oy as f32 / s, (ox + 1) as f32 / s, (oy + 1) as f32 / s],
    ))
}

/// Cache key for decoded tiles: a template change must not show old tiles.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Hash of the URL template (not the template itself, which can be long).
    pub template_hash: u64,
    /// The tile.
    pub tile: TileId,
}

/// Stable-per-process hash of a URL template.
pub fn template_hash(template: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    template.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basemaps_on_one_host_do_not_share_a_cache_directory() {
        let light = "https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}.png";
        let dark = "https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png";
        assert_ne!(cache_dir_name(light), cache_dir_name(dark));
        assert!(!cache_dir_name(light).starts_with("{s}"));
        assert_eq!(cache_dir_name(light), cache_dir_name(light));
        let t = TileId { z: 1, x: 0, y: 1 };
        let root = std::path::Path::new("/c");
        assert_ne!(disk_path(root, light, t), disk_path(root, dark, t));
    }

    fn t(z: u8, x: u32, y: u32) -> TileId {
        TileId { z, x, y }
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut c = Lru::new(2);
        c.insert("a", 1);
        c.insert("b", 2);
        assert_eq!(c.get(&"a"), Some(&1)); // a is now newest
        c.insert("c", 3);
        assert!(c.contains(&"a") && c.contains(&"c") && !c.contains(&"b"));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn lru_replace_does_not_grow() {
        let mut c = Lru::new(2);
        c.insert(1, "x");
        c.insert(1, "y");
        c.insert(2, "z");
        assert_eq!(c.len(), 2);
        assert_eq!(c.peek(&1), Some(&"y"));
    }

    #[test]
    fn host_extraction() {
        assert_eq!(host_of("https://tile.openstreetmap.org/{z}/{x}/{y}.png"), "tile.openstreetmap.org");
        assert_eq!(host_of("http://localhost:8080/t/{z}/{x}/{y}.png"), "localhost_8080");
        assert_eq!(host_of("https://user:pw@tiles.example.com/x"), "tiles.example.com");
        assert_eq!(host_of(""), "unknown");
        assert_eq!(host_of("https://../{z}"), "..");
    }

    #[test]
    fn disk_layout() {
        let p = disk_path(
            Path::new("/c/theta/tiles"),
            "https://tile.openstreetmap.org/{z}/{x}/{y}.png",
            t(5, 17, 11),
        );
        let dir = cache_dir_name("https://tile.openstreetmap.org/{z}/{x}/{y}.png");
        assert!(dir.starts_with("tile.openstreetmap.org-"), "{dir}");
        assert_eq!(p, PathBuf::from(format!("/c/theta/tiles/{dir}/5/17/11.png")));
    }

    #[test]
    fn freshness_window() {
        let now = SystemTime::now();
        assert!(is_fresh(now - Duration::from_secs(3600), now));
        assert!(is_fresh(now - Duration::from_secs(6 * 24 * 3600), now));
        assert!(!is_fresh(now - Duration::from_secs(8 * 24 * 3600), now));
        assert!(is_fresh(now + Duration::from_secs(100), now));
    }

    #[test]
    fn request_planning_skips_have_inflight_failed_and_caps() {
        let wanted: Vec<TileId> = (0..8).map(|x| t(3, x, 0)).collect();
        let have = |x: TileId| x.x == 0;
        let in_flight: HashSet<TileId> = [t(3, 1, 0)].into();
        let failed: HashSet<TileId> = [t(3, 2, 0)].into();
        let plan = plan_requests(&wanted, have, &in_flight, &failed, 3);
        assert_eq!(plan, vec![t(3, 3, 0), t(3, 4, 0), t(3, 5, 0)]);
    }

    #[test]
    fn ancestor_regions() {
        let (p, uv) = ancestor_region(t(5, 17, 11), 1).unwrap();
        assert_eq!(p, t(4, 8, 5));
        assert_eq!(uv, [0.5, 0.5, 1.0, 1.0]);
        let (p, uv) = ancestor_region(t(5, 16, 10), 2).unwrap();
        assert_eq!(p, t(3, 4, 2));
        assert_eq!(uv, [0.0, 0.5, 0.25, 0.75]);
        assert!(ancestor_region(t(1, 0, 0), 2).is_none());
        assert!(ancestor_region(t(3, 0, 0), 0).is_none());
    }

    #[test]
    fn template_change_changes_key() {
        let a = TileKey { template_hash: template_hash("https://a/{z}"), tile: t(1, 0, 0) };
        let b = TileKey { template_hash: template_hash("https://b/{z}"), tile: t(1, 0, 0) };
        assert_ne!(a, b);
    }
}
