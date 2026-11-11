//! Consoles survive restarts: one JSON file per console in `<config_dir>/consoles/`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedConsole {
    /// Stable key; also the file name.
    pub key: String,
    /// Position in the tab strip.
    pub order: usize,
    pub title: String,
    pub connection_id: Option<String>,
    pub text: String,
    /// Cursor as a char index.
    pub cursor: usize,
    #[serde(default)]
    pub active: bool,
    /// The `.sql` file this console was saved to or opened from.
    #[serde(default)]
    pub file: Option<PathBuf>,
}

pub fn consoles_dir() -> PathBuf {
    theta_ui::settings::config_dir().join("consoles")
}

fn file_for(dir: &Path, key: &str) -> PathBuf {
    let safe: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    dir.join(format!("{safe}.json"))
}

pub fn save(dir: &Path, c: &SavedConsole) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = serde_json::to_string(c).map_err(std::io::Error::other)?;
    theta_ui::settings::write_atomic(&file_for(dir, &c.key), body.as_bytes())
}

pub fn remove(dir: &Path, key: &str) {
    let _ = std::fs::remove_file(file_for(dir, key));
}

/// All saved consoles in tab order; unreadable files are skipped.
pub fn load_all(dir: &Path) -> Vec<SavedConsole> {
    let Ok(read) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<SavedConsole> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    v.sort_by_key(|c| c.order);
    v
}

/// How many closed consoles "Reopen Closed Tab" can bring back.
pub const CLOSED_LIMIT: usize = 20;

/// A closed console on the reopen stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Closed {
    /// Nanoseconds since the epoch when it was closed; the stack order.
    closed_at: u128,
    console: SavedConsole,
}

pub fn closed_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("closed-consoles")
}

fn closed_all(dir: &Path) -> Vec<(PathBuf, Closed)> {
    let Ok(read) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<(PathBuf, Closed)> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let c: Closed = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some((e.path(), c))
        })
        .collect();
    v.sort_by(|a, b| a.1.closed_at.cmp(&b.1.closed_at).then_with(|| a.1.console.key.cmp(&b.1.console.key)));
    v
}

/// Put a console on the reopen stack (newest on top); the oldest beyond [`CLOSED_LIMIT`] is
/// deleted. Whitespace-only consoles are not worth bringing back and are dropped.
pub fn push_closed(dir: &Path, c: &SavedConsole) -> std::io::Result<()> {
    if c.text.trim().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let closed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let body =
        serde_json::to_string(&Closed { closed_at, console: c.clone() }).map_err(std::io::Error::other)?;
    theta_ui::settings::write_atomic(&file_for(dir, &c.key), body.as_bytes())?;
    let all = closed_all(dir);
    let excess = all.len().saturating_sub(CLOSED_LIMIT);
    for (path, _) in all.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// Take the most recently closed console off the stack.
pub fn pop_closed(dir: &Path) -> Option<SavedConsole> {
    let (path, closed) = closed_all(dir).pop()?;
    let _ = std::fs::remove_file(path);
    Some(closed.console)
}

/// The reopen stack, newest first.
pub fn list_closed(dir: &Path) -> Vec<SavedConsole> {
    closed_all(dir).into_iter().rev().map(|(_, c)| c.console).collect()
}

/// A fresh unique key.
pub fn new_key() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(key: &str, order: usize, text: &str) -> SavedConsole {
        SavedConsole {
            key: key.into(),
            order,
            title: format!("console {order}"),
            connection_id: Some("conn".into()),
            text: text.into(),
            cursor: 3,
            active: order == 1,
            file: None,
        }
    }

    #[test]
    fn round_trip_in_order() {
        let d = tempfile::tempdir().unwrap();
        save(d.path(), &c("b", 1, "select 2")).unwrap();
        save(d.path(), &c("a", 0, "select 1")).unwrap();
        let all = load_all(d.path());
        assert_eq!(all.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["select 1", "select 2"]);
        assert!(all[1].active);
    }

    #[test]
    fn remove_and_garbage() {
        let d = tempfile::tempdir().unwrap();
        save(d.path(), &c("a", 0, "x")).unwrap();
        std::fs::write(d.path().join("junk.json"), "{").unwrap();
        assert_eq!(load_all(d.path()).len(), 1);
        remove(d.path(), "a");
        assert!(load_all(d.path()).is_empty());
    }

    #[test]
    fn closed_consoles_are_a_bounded_stack_newest_first() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..(CLOSED_LIMIT + 5) {
            push_closed(d.path(), &c(&format!("k{i}"), i, &format!("select {i}"))).unwrap();
        }
        let listed = list_closed(d.path());
        assert_eq!(listed.len(), CLOSED_LIMIT, "the oldest five are evicted");
        assert_eq!(listed[0].text, format!("select {}", CLOSED_LIMIT + 4));
        assert_eq!(listed.last().unwrap().text, "select 5");
        let top = pop_closed(d.path()).unwrap();
        assert_eq!(top.text, format!("select {}", CLOSED_LIMIT + 4));
        assert_eq!(top.cursor, 3);
        assert_eq!(top.connection_id.as_deref(), Some("conn"));
        assert_eq!(list_closed(d.path()).len(), CLOSED_LIMIT - 1);
        // Evicted files are really gone, kept files are really there.
        let files = std::fs::read_dir(d.path()).unwrap().count();
        assert_eq!(files, CLOSED_LIMIT - 1);
    }

    #[test]
    fn empty_consoles_are_not_kept_and_an_empty_stack_pops_nothing() {
        let d = tempfile::tempdir().unwrap();
        push_closed(d.path(), &c("a", 0, "  \n")).unwrap();
        assert!(pop_closed(d.path()).is_none());
        assert!(pop_closed(&d.path().join("missing")).is_none());
    }

    #[test]
    fn keys_cannot_escape_dir() {
        let d = tempfile::tempdir().unwrap();
        let p = file_for(d.path(), "../../etc/x");
        assert!(p.starts_with(d.path()));
    }
}
