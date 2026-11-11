//! Connection folders and ordering: the pure model behind the explorer's groups.
//!
//! The saved connections form one ordered list (`connections.json` keeps it in that order); a folder
//! is a named group that connections can belong to. Folders are one level deep: a folder holds
//! connections, not other folders. A connection's position *within its group* is its order in the
//! list, so "move up" swaps it with the previous connection of the same group and never reshuffles
//! the others.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// Remembered across restarts: a folder the user collapsed stays collapsed.
    #[serde(default)]
    pub collapsed: bool,
}

/// Folders (in display order) and which folder each connection is in. A connection that is absent
/// from `folder_of` is at the top level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layout {
    pub folders: Vec<Folder>,
    pub folder_of: HashMap<String, String>,
}

pub fn new_folder_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl Layout {
    pub fn folder(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    pub fn folder_name_of(&self, conn: &str) -> Option<&str> {
        self.folder_of.get(conn).and_then(|f| self.folder(f)).map(|f| f.name.as_str())
    }

    /// `Folder / Name`, or just the name for a top-level connection. What pickers show and match.
    pub fn display_name(&self, conn: &str, name: &str) -> String {
        match self.folder_name_of(conn) {
            Some(f) => format!("{f} / {name}"),
            None => name.to_string(),
        }
    }

    /// Whether `name` is free (case-insensitively), ignoring the folder `except`.
    pub fn name_is_free(&self, name: &str, except: Option<&str>) -> bool {
        let n = name.trim();
        !n.is_empty()
            && !self
                .folders
                .iter()
                .any(|f| Some(f.id.as_str()) != except && f.name.trim().eq_ignore_ascii_case(n))
    }

    pub fn add_folder(&mut self, name: &str) -> Option<String> {
        if !self.name_is_free(name, None) {
            return None;
        }
        let id = new_folder_id();
        self.folders.push(Folder { id: id.clone(), name: name.trim().to_string(), collapsed: false });
        Some(id)
    }

    pub fn rename_folder(&mut self, id: &str, name: &str) -> bool {
        if !self.name_is_free(name, Some(id)) {
            return false;
        }
        match self.folders.iter_mut().find(|f| f.id == id) {
            Some(f) => {
                f.name = name.trim().to_string();
                true
            }
            None => false,
        }
    }

    /// Remove the folder; its connections move to the top level (they are never deleted with it).
    pub fn delete_folder(&mut self, id: &str) {
        self.folders.retain(|f| f.id != id);
        self.folder_of.retain(|_, f| f != id);
    }

    pub fn set_collapsed(&mut self, id: &str, collapsed: bool) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == id) {
            f.collapsed = collapsed;
        }
    }

    pub fn move_to(&mut self, conn: &str, folder: Option<&str>) {
        match folder.filter(|f| self.folder(f).is_some()) {
            Some(f) => {
                self.folder_of.insert(conn.to_string(), f.to_string());
            }
            None => {
                self.folder_of.remove(conn);
            }
        }
    }

    /// Move folder `id` one place earlier (`-1`) or later (`1`). Returns whether it moved.
    pub fn move_folder(&mut self, id: &str, delta: i32) -> bool {
        let Some(i) = self.folders.iter().position(|f| f.id == id) else { return false };
        let j = i as i64 + delta as i64;
        if j < 0 || j >= self.folders.len() as i64 {
            return false;
        }
        self.folders.swap(i, j as usize);
        true
    }

    /// Drop assignments to folders that no longer exist (a hand-edited or damaged file).
    pub fn sanitize(&mut self) {
        let known: std::collections::HashSet<&str> = self.folders.iter().map(|f| f.id.as_str()).collect();
        let kept: HashMap<String, String> = self
            .folder_of
            .iter()
            .filter(|(_, f)| known.contains(f.as_str()))
            .map(|(c, f)| (c.clone(), f.clone()))
            .collect();
        self.folder_of = kept;
        let mut seen = std::collections::HashSet::new();
        self.folders.retain(|f| seen.insert(f.id.clone()));
    }

    /// Move connection `id` one place earlier or later *within its group* in `order` (the full list
    /// of connection ids). Returns whether anything moved.
    pub fn move_connection(&self, order: &mut [String], id: &str, delta: i32) -> bool {
        let Some(i) = order.iter().position(|c| c == id) else { return false };
        let group = self.folder_of.get(id);
        let same = |c: &String| self.folder_of.get(c) == group;
        let mut j = i as i64;
        loop {
            j += delta.signum() as i64;
            if j < 0 || j >= order.len() as i64 {
                return false;
            }
            if same(&order[j as usize]) {
                break;
            }
        }
        order.swap(i, j as usize);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn layout() -> (Layout, String, String) {
        let mut l = Layout::default();
        let prod = l.add_folder("Prod").unwrap();
        let dev = l.add_folder("Dev").unwrap();
        (l, prod, dev)
    }

    #[test]
    fn folder_names_are_unique_and_non_empty() {
        let (mut l, prod, _) = layout();
        assert!(l.add_folder("  prod ").is_none(), "case-insensitive duplicate");
        assert!(l.add_folder("   ").is_none());
        assert!(l.rename_folder(&prod, "PROD"), "renaming to its own name differing in case is fine");
        assert!(!l.rename_folder(&prod, "dev"));
        assert_eq!(l.folder(&prod).unwrap().name, "PROD");
    }

    #[test]
    fn display_name_has_the_folder_prefix() {
        let (mut l, prod, _) = layout();
        l.move_to("a", Some(&prod));
        assert_eq!(l.display_name("a", "db1"), "Prod / db1");
        assert_eq!(l.display_name("b", "db2"), "db2");
    }

    #[test]
    fn deleting_a_folder_returns_its_connections_to_the_top_level() {
        let (mut l, prod, dev) = layout();
        l.move_to("a", Some(&prod));
        l.move_to("b", Some(&dev));
        l.delete_folder(&prod);
        assert_eq!(l.folder_name_of("a"), None);
        assert_eq!(l.folder_name_of("b"), Some("Dev"));
        assert_eq!(l.folders.len(), 1);
    }

    #[test]
    fn moving_to_an_unknown_folder_means_top_level() {
        let (mut l, prod, _) = layout();
        l.move_to("a", Some(&prod));
        l.move_to("a", Some("missing"));
        assert!(l.folder_of.is_empty());
    }

    #[test]
    fn move_within_group_skips_other_groups() {
        let (mut l, prod, _) = layout();
        // order: a(prod) x(top) b(prod) y(top) c(prod)
        let mut order = ids(&["a", "x", "b", "y", "c"]);
        for c in ["a", "b", "c"] {
            l.move_to(c, Some(&prod));
        }
        assert!(l.move_connection(&mut order, "b", -1));
        assert_eq!(order, ids(&["b", "x", "a", "y", "c"]), "b swaps with the previous member of Prod, a");
        assert!(!l.move_connection(&mut order, "b", -1), "already first in its group");
        assert!(l.move_connection(&mut order, "x", 1));
        assert_eq!(order, ids(&["b", "y", "a", "x", "c"]), "x is alone at the top level and swaps with y");
        assert!(!l.move_connection(&mut order, "c", 1));
        assert!(!l.move_connection(&mut order, "nope", 1));
    }

    #[test]
    fn move_folder_reorders() {
        let (mut l, prod, dev) = layout();
        assert!(l.move_folder(&dev, -1));
        assert_eq!(l.folders[0].id, dev);
        assert!(!l.move_folder(&dev, -1));
        assert!(l.move_folder(&prod, -1));
        assert_eq!(l.folders[0].id, prod);
    }

    #[test]
    fn sanitize_drops_dangling_assignments_and_duplicate_folders() {
        let (mut l, prod, _) = layout();
        l.folder_of.insert("a".into(), "gone".into());
        l.folder_of.insert("b".into(), prod.clone());
        l.folders.push(l.folders[0].clone());
        l.sanitize();
        assert_eq!(l.folder_of.len(), 1);
        assert_eq!(l.folders.len(), 2);
    }
}
