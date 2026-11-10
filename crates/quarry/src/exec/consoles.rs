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
    quarry_ui::settings::config_dir().join("consoles")
}

fn file_for(dir: &Path, key: &str) -> PathBuf {
    let safe: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    dir.join(format!("{safe}.json"))
}

pub fn save(dir: &Path, c: &SavedConsole) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let body = serde_json::to_string(c).map_err(std::io::Error::other)?;
    quarry_ui::settings::write_atomic(&file_for(dir, &c.key), body.as_bytes())
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
    fn keys_cannot_escape_dir() {
        let d = tempfile::tempdir().unwrap();
        let p = file_for(d.path(), "../../etc/x");
        assert!(p.starts_with(d.path()));
    }
}
