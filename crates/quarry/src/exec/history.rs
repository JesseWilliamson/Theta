//! Statement history in `<config_dir>/history.jsonl`, newest last, capped.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const CAP: usize = 5000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unix milliseconds.
    pub at: u64,
    pub connection: String,
    pub sql: String,
    pub duration_ms: u64,
    pub rows: Option<u64>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn history_path() -> PathBuf {
    quarry_ui::settings::config_dir().join("history.jsonl")
}

pub struct HistoryStore {
    path: Option<PathBuf>,
    entries: Vec<HistoryEntry>,
    appended_since_compact: usize,
}

impl HistoryStore {
    pub fn load(path: Option<PathBuf>) -> Self {
        let mut entries: Vec<HistoryEntry> = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default();
        if entries.len() > CAP {
            entries.drain(..entries.len() - CAP);
        }
        Self { path, entries, appended_since_compact: 0 }
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn push(&mut self, entry: HistoryEntry) {
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path)
                && let Ok(line) = serde_json::to_string(&entry)
            {
                let _ = writeln!(f, "{line}");
            }
        }
        self.entries.push(entry);
        self.appended_since_compact += 1;
        if self.entries.len() > CAP {
            self.entries.drain(..self.entries.len() - CAP);
        }
        if self.appended_since_compact >= 500 {
            self.compact();
        }
    }

    /// Rewrite the file with only the retained entries.
    pub fn compact(&mut self) {
        self.appended_since_compact = 0;
        let Some(path) = &self.path else { return };
        let body: String =
            self.entries.iter().filter_map(|e| serde_json::to_string(e).ok()).map(|l| l + "\n").collect();
        let _ = quarry_ui::settings::write_atomic(path, body.as_bytes());
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        if let Some(p) = &self.path {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Fuzzy-filtered entries, newest first.
    pub fn search<'a>(&'a self, query: &str) -> Vec<&'a HistoryEntry> {
        let q = query.trim();
        let mut out: Vec<(_, &HistoryEntry)> = self
            .entries
            .iter()
            .rev()
            .filter_map(|e| {
                if q.is_empty() {
                    return Some((0, e));
                }
                quarry_ui::fuzzy::fuzzy_match(q, &e.sql).map(|m| (m.score, e))
            })
            .collect();
        if !q.is_empty() {
            out.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        out.into_iter().map(|(_, e)| e).collect()
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(sql: &str, at: u64) -> HistoryEntry {
        HistoryEntry {
            at,
            connection: "c".into(),
            sql: sql.into(),
            duration_ms: 1,
            rows: Some(1),
            ok: true,
            error: None,
        }
    }

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.jsonl");
        let mut s = HistoryStore::load(Some(p.clone()));
        s.push(e("select 1", 1));
        s.push(e("select 2", 2));
        let again = HistoryStore::load(Some(p));
        assert_eq!(again.entries().len(), 2);
        assert_eq!(again.entries()[1].sql, "select 2");
    }

    #[test]
    fn cap_is_enforced_on_load_and_push() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.jsonl");
        let mut body = String::new();
        for i in 0..CAP + 10 {
            body += &(serde_json::to_string(&e(&format!("s{i}"), i as u64)).unwrap() + "\n");
        }
        std::fs::write(&p, body).unwrap();
        let mut s = HistoryStore::load(Some(p));
        assert_eq!(s.entries().len(), CAP);
        assert_eq!(s.entries()[0].sql, "s10");
        s.push(e("last", 0));
        assert_eq!(s.entries().len(), CAP);
    }

    #[test]
    fn search_is_fuzzy_newest_first() {
        let mut s = HistoryStore::load(None);
        s.push(e("select * from customers", 1));
        s.push(e("select * from orders", 2));
        assert_eq!(s.search("").first().unwrap().sql, "select * from orders");
        let r = s.search("cust");
        assert_eq!(r.len(), 1);
        assert!(r[0].sql.contains("customers"));
    }

    #[test]
    fn tolerates_garbage_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.jsonl");
        std::fs::write(&p, "not json\n").unwrap();
        assert!(HistoryStore::load(Some(p)).entries().is_empty());
    }
}
