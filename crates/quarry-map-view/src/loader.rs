//! Background tile fetching.
//!
//! A small pool of plain OS threads (blocking HTTP, no async runtime) pulls from a queue
//! that the panel *replaces* on every viewport change. That is the cancellation story:
//! tiles that scrolled out of view are simply no longer in the queue, so they are never
//! started; a request already on the wire finishes (bounded by the timeout) and lands in
//! the caches where it will be useful when the user pans back.

use crate::decode::decode_tile;
use crate::tiles::{MAX_IN_FLIGHT, TileKey, disk_path, is_fresh, template_hash};
use gpui_kit::RenderImage;
use quarry_geo::TileId;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime};

/// Outcome of one tile request.
pub struct TileResult {
    /// Which tile (including the template it was fetched for).
    pub key: TileKey,
    /// Decoded image, or a short error for logs/tooltips.
    pub image: Result<Arc<RenderImage>, String>,
}

struct Queue {
    template: String,
    wanted: Vec<TileId>,
    in_flight: HashSet<TileId>,
    shutdown: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    results: async_channel::Sender<TileResult>,
    root: PathBuf,
    user_agent: String,
}

/// Handle owned by the panel.
pub struct TileLoader {
    shared: Arc<Shared>,
    /// Completed requests; awaited by a GPUI task in the panel.
    pub results: async_channel::Receiver<TileResult>,
    workers_started: bool,
}

/// `Quarry/<version> (+repo url)`, as the OSM tile usage policy requires.
pub fn user_agent() -> String {
    format!("Quarry/{} (+https://github.com/JesseWilliamson/Quarry)", env!("CARGO_PKG_VERSION"))
}

impl TileLoader {
    /// A loader caching under `root` (see `tiles::cache_root`).
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = async_channel::unbounded();
        TileLoader {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue {
                    template: String::new(),
                    wanted: Vec::new(),
                    in_flight: HashSet::new(),
                    shutdown: false,
                }),
                wake: Condvar::new(),
                results: tx,
                root,
                user_agent: user_agent(),
            }),
            results: rx,
            workers_started: false,
        }
    }

    /// Replace the set of tiles to fetch (nearest the centre first). Anything not listed
    /// and not yet started is dropped.
    pub fn set_wanted(&mut self, template: &str, wanted: Vec<TileId>) {
        if !self.workers_started && !wanted.is_empty() {
            self.workers_started = true;
            for i in 0..MAX_IN_FLIGHT {
                let shared = self.shared.clone();
                // A failure to spawn only reduces parallelism; the basemap still works.
                let _ = std::thread::Builder::new()
                    .name(format!("quarry-tiles-{i}"))
                    .spawn(move || worker(shared));
            }
        }
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.template = template.to_string();
        // A tile already on the wire must not be queued a second time.
        q.wanted = wanted.into_iter().filter(|t| !q.in_flight.contains(t)).collect();
        drop(q);
        self.shared.wake.notify_all();
    }
}

impl Drop for TileLoader {
    fn drop(&mut self) {
        if let Ok(mut q) = self.shared.queue.lock() {
            q.shutdown = true;
            q.wanted.clear();
        }
        self.shared.wake.notify_all();
    }
}

fn worker(shared: Arc<Shared>) {
    loop {
        let (tile, template) = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if q.shutdown {
                    return;
                }
                let pos = q.wanted.iter().position(|t| !q.in_flight.contains(t));
                if let Some(pos) = pos {
                    let tile = q.wanted.remove(pos);
                    q.in_flight.insert(tile);
                    break (tile, q.template.clone());
                }
                q = shared.wake.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        let image = fetch(&shared, &template, tile);
        if let Ok(mut q) = shared.queue.lock() {
            q.in_flight.remove(&tile);
        }
        let key = TileKey { template_hash: template_hash(&template), tile };
        if shared.results.send_blocking(TileResult { key, image }).is_err() {
            return; // the panel is gone
        }
    }
}

fn fetch(shared: &Shared, template: &str, tile: TileId) -> Result<Arc<RenderImage>, String> {
    let path = disk_path(&shared.root, template, tile);
    let meta = std::fs::metadata(&path).ok();
    let fresh = meta.as_ref().and_then(|m| m.modified().ok()).is_some_and(|m| is_fresh(m, SystemTime::now()));
    if fresh {
        if let Some(img) = std::fs::read(&path).ok().and_then(|b| decode_tile(&b).ok()) {
            return Ok(img);
        }
        // Corrupt cache entry (interrupted write by an older build, disk full): refetch.
        let _ = std::fs::remove_file(&path);
    }
    match download(&shared.user_agent, &tile.url(template)) {
        Ok(bytes) => {
            let img = decode_tile(&bytes)?;
            write_cache(&path, &bytes);
            Ok(img)
        }
        Err(e) => {
            // Offline but we have an old copy: stale tiles beat a blank map.
            if meta.is_some()
                && let Some(img) = std::fs::read(&path).ok().and_then(|b| decode_tile(&b).ok())
            {
                return Ok(img);
            }
            Err(e)
        }
    }
}

fn download(user_agent: &str, url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(user_agent)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    resp.body_mut().with_config().limit(4 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())
}

/// Write through a temp file so a crash never leaves a truncated tile that would then be
/// served as "fresh". Errors are ignored: the cache is an optimisation.
fn write_cache(path: &std::path::Path, bytes: &[u8]) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn user_agent_names_the_app_and_repo() {
        let ua = user_agent();
        assert!(ua.starts_with("Quarry/") && ua.contains("(+https://github.com/JesseWilliamson/Quarry)"));
    }

    #[test]
    fn fresh_disk_entry_is_served_without_network() {
        let dir = tempfile::tempdir().unwrap();
        // An unroutable host: if the loader tried the network this test would error.
        let template = "http://127.0.0.1:9/{z}/{x}/{y}.png";
        let tile = TileId { z: 3, x: 1, y: 2 };
        let path = disk_path(dir.path(), template, tile);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png()).unwrap();
        let shared = Shared {
            queue: Mutex::new(Queue {
                template: template.into(),
                wanted: vec![],
                in_flight: HashSet::new(),
                shutdown: false,
            }),
            wake: Condvar::new(),
            results: async_channel::unbounded().0,
            root: dir.path().to_path_buf(),
            user_agent: user_agent(),
        };
        assert!(fetch(&shared, template, tile).is_ok());
    }

    #[test]
    fn unreachable_host_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let shared = Shared {
            queue: Mutex::new(Queue {
                template: String::new(),
                wanted: vec![],
                in_flight: HashSet::new(),
                shutdown: false,
            }),
            wake: Condvar::new(),
            results: async_channel::unbounded().0,
            root: dir.path().to_path_buf(),
            user_agent: user_agent(),
        };
        let r = fetch(&shared, "http://127.0.0.1:9/{z}/{x}/{y}.png", TileId { z: 0, x: 0, y: 0 });
        assert!(r.is_err());
    }

    #[test]
    fn stale_entry_is_the_offline_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let template = "http://127.0.0.1:9/{z}/{x}/{y}.png";
        let tile = TileId { z: 0, x: 0, y: 0 };
        let path = disk_path(dir.path(), template, tile);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png()).unwrap();
        let old = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        let shared = Shared {
            queue: Mutex::new(Queue {
                template: template.into(),
                wanted: vec![],
                in_flight: HashSet::new(),
                shutdown: false,
            }),
            wake: Condvar::new(),
            results: async_channel::unbounded().0,
            root: dir.path().to_path_buf(),
            user_agent: user_agent(),
        };
        assert!(fetch(&shared, template, tile).is_ok());
    }
}
