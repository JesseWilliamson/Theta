//! Local history: snapshots of a console's text, kept on disk so that an overwritten or lost statement
//! can be found again, and so that the text survives a crash.
//!
//! Layout: `<config>/history/<console key>/<unix ms>-<kind>.sql`, one plain file per snapshot. A snapshot is
//! written to a temporary name and renamed into place (`write_atomic`), so a process killed in the middle of
//! a write leaves either the old state or the complete new file, never a torn one; readers ignore anything
//! that does not match the naming scheme, including leftover temporary files.
//!
//! Policy, all of it in this file so it can be tested without a window:
//! * a snapshot identical to the newest one is not written again, and blank text is never written;
//! * *edit* snapshots are coalesced: while the newest snapshot is an edit younger than [`COALESCE_MS`], the
//!   next edit replaces it instead of adding another, so a long typing session leaves one snapshot per
//!   fifteen seconds of work, not one per pause; a *run* is always its own snapshot (what was actually
//!   executed), and so is a *restore* (the text that was replaced);
//! * a text over [`MAX_SNAPSHOT_BYTES`] is not stored (and the caller is told);
//! * pruning keeps each console under [`MAX_PER_CONSOLE`] snapshots and [`MAX_CONSOLE_BYTES`], and all of
//!   history under [`MAX_TOTAL_BYTES`], always dropping the oldest first and never the newest of a console.

use std::path::{Path, PathBuf};

pub const COALESCE_MS: u64 = 15_000;
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_PER_CONSOLE: usize = 100;
pub const MAX_CONSOLE_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Taken after a pause in typing.
    Edit,
    /// The text of the console at the moment a statement was run.
    Run,
    /// The text a restore replaced, so a restore can itself be undone from history.
    Restore,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::Edit => "edit",
            Kind::Run => "run",
            Kind::Restore => "restore",
        }
    }

    fn from_tag(t: &str) -> Option<Kind> {
        Some(match t {
            "edit" => Kind::Edit,
            "run" => Kind::Run,
            "restore" => Kind::Restore,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Edit => "edited",
            Kind::Run => "run",
            Kind::Restore => "before restore",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub at_ms: u64,
    pub kind: Kind,
    pub bytes: u64,
    /// File name, the handle for [`LocalHistory::read`].
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Written,
    /// The newest edit snapshot was replaced.
    Coalesced,
    /// Same as the newest snapshot, or blank.
    Skipped,
    /// Same as the newest snapshot, which was an edit; it is now marked as a run, so the next edit cannot
    /// coalesce into it and the version that was run stays in the list.
    Promoted,
    TooLarge,
}

pub struct LocalHistory {
    root: PathBuf,
    max_total: u64,
    /// Only snapshots taken at or after this time can be coalesced into. A restart is a new session: the
    /// text as it was when the previous process died must not be overwritten by the first edit after it.
    session_start_ms: u64,
}

fn safe_key(key: &str) -> String {
    key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

fn parse_name(name: &str) -> Option<(u64, Kind)> {
    let stem = name.strip_suffix(".sql")?;
    let (ms, tag) = stem.split_once('-')?;
    Some((ms.parse().ok()?, Kind::from_tag(tag)?))
}

/// The first non-blank line, trimmed and cut for a list row.
pub fn summary(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let cut: String = line.chars().take(90).collect();
    if line.chars().count() > 90 { format!("{cut}…") } else { cut }
}

impl LocalHistory {
    pub fn new(root: PathBuf) -> Self {
        Self { root, max_total: MAX_TOTAL_BYTES, session_start_ms: 0 }
    }

    pub fn session_from(mut self, start_ms: u64) -> Self {
        self.session_start_ms = start_ms;
        self
    }

    /// A smaller total cap, for tests that must not write tens of megabytes to prove the pruning.
    #[cfg(test)]
    fn with_total_cap(mut self, bytes: u64) -> Self {
        self.max_total = bytes;
        self
    }

    fn dir(&self, key: &str) -> PathBuf {
        self.root.join(safe_key(key))
    }

    /// Snapshots of a console, newest first.
    pub fn list(&self, key: &str) -> Vec<Meta> {
        let Ok(read) = std::fs::read_dir(self.dir(key)) else { return Vec::new() };
        let mut v: Vec<Meta> = read
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().to_string_lossy().into_owned();
                let (at_ms, kind) = parse_name(&id)?;
                Some(Meta { at_ms, kind, bytes: e.metadata().map(|m| m.len()).unwrap_or(0), id })
            })
            .collect();
        v.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then(b.id.cmp(&a.id)));
        v
    }

    pub fn read(&self, key: &str, id: &str) -> Option<String> {
        // `id` comes from `list`, but never trust a name that could leave the directory.
        parse_name(id)?;
        std::fs::read_to_string(self.dir(key).join(id)).ok()
    }

    /// Record `text` for the console `key` at `now_ms`.
    pub fn record(&self, key: &str, text: &str, kind: Kind, now_ms: u64) -> std::io::Result<Recorded> {
        if text.trim().is_empty() {
            return Ok(Recorded::Skipped);
        }
        if text.len() > MAX_SNAPSHOT_BYTES {
            return Ok(Recorded::TooLarge);
        }
        let dir = self.dir(key);
        let existing = self.list(key);
        if let Some(newest) = existing.first()
            && self.read(key, &newest.id).as_deref() == Some(text)
        {
            if kind == Kind::Run && newest.kind == Kind::Edit {
                std::fs::rename(dir.join(&newest.id), dir.join(format!("{:013}-run.sql", newest.at_ms)))?;
                return Ok(Recorded::Promoted);
            }
            return Ok(Recorded::Skipped);
        }
        std::fs::create_dir_all(&dir)?;
        // Coalesce into the newest edit snapshot while it is recent.
        let replace = existing.first().filter(|m| {
            kind == Kind::Edit
                && m.kind == Kind::Edit
                && m.at_ms >= self.session_start_ms
                && now_ms.saturating_sub(m.at_ms) < COALESCE_MS
        });
        let (at, outcome) = match replace {
            Some(m) => (m.at_ms, Recorded::Coalesced),
            None => (now_ms.max(existing.first().map_or(0, |m| m.at_ms + 1)), Recorded::Written),
        };
        let path = dir.join(format!("{at:013}-{}.sql", kind.tag()));
        theta_ui::settings::write_atomic(&path, text.as_bytes())?;
        self.prune(key);
        Ok(outcome)
    }

    /// Enforce the per-console and total caps, oldest first, keeping the newest snapshot of each console.
    pub fn prune(&self, key: &str) {
        let metas = self.list(key);
        let (mut count, mut bytes) = (metas.len(), metas.iter().map(|m| m.bytes).sum::<u64>());
        for m in metas.iter().rev() {
            if count <= 1 || (count <= MAX_PER_CONSOLE && bytes <= MAX_CONSOLE_BYTES) {
                break;
            }
            if std::fs::remove_file(self.dir(key).join(&m.id)).is_ok() {
                count -= 1;
                bytes = bytes.saturating_sub(m.bytes);
            }
        }
        self.prune_total();
    }

    fn prune_total(&self) {
        let Ok(consoles) = std::fs::read_dir(&self.root) else { return };
        let mut all: Vec<(u64, PathBuf, u64, bool)> = Vec::new(); // (at, path, bytes, is_newest_of_console)
        for c in consoles.flatten().filter(|c| c.path().is_dir()) {
            let key = c.file_name().to_string_lossy().into_owned();
            for (i, m) in self.list(&key).into_iter().enumerate() {
                all.push((m.at_ms, c.path().join(&m.id), m.bytes, i == 0));
            }
        }
        let mut total: u64 = all.iter().map(|a| a.2).sum();
        all.sort_by_key(|a| a.0);
        for (_, path, bytes, newest) in all {
            if total <= self.max_total {
                break;
            }
            if !newest && std::fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(bytes);
            }
        }
    }

    /// Forget a console's history (when its tab is closed for good).
    pub fn remove(&self, key: &str) {
        let _ = std::fs::remove_dir_all(self.dir(key));
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, LocalHistory) {
        let d = tempfile::tempdir().unwrap();
        let h = LocalHistory::new(d.path().to_path_buf());
        (d, h)
    }

    #[test]
    fn snapshots_are_listed_newest_first_and_read_back() {
        let (_d, h) = store();
        assert_eq!(h.record("c1", "select 1", Kind::Run, 1_000).unwrap(), Recorded::Written);
        assert_eq!(h.record("c1", "select 2", Kind::Run, 2_000).unwrap(), Recorded::Written);
        let l = h.list("c1");
        assert_eq!(l.iter().map(|m| m.at_ms).collect::<Vec<_>>(), [2_000, 1_000]);
        assert_eq!(h.read("c1", &l[0].id).unwrap(), "select 2");
        assert_eq!(h.read("c1", &l[1].id).unwrap(), "select 1");
        assert!(h.list("other").is_empty());
    }

    #[test]
    fn identical_and_blank_text_is_not_recorded() {
        let (_d, h) = store();
        h.record("c", "select 1", Kind::Edit, 0).unwrap();
        assert_eq!(h.record("c", "select 1", Kind::Edit, 99_000).unwrap(), Recorded::Skipped);
        assert_eq!(h.record("c", "  \n\t ", Kind::Edit, 99_000).unwrap(), Recorded::Skipped);
        assert_eq!(h.list("c").len(), 1);
    }

    #[test]
    fn running_the_text_that_was_just_snapshotted_marks_it_as_a_run_and_keeps_it_apart_from_later_edits() {
        let (_d, h) = store();
        h.record("c", "select 1", Kind::Edit, 1_000).unwrap();
        assert_eq!(h.record("c", "select 1", Kind::Run, 2_000).unwrap(), Recorded::Promoted);
        let l = h.list("c");
        assert_eq!((l.len(), l[0].kind, l[0].at_ms), (1, Kind::Run, 1_000));
        // The edit that follows is a new snapshot, not folded into the run.
        assert_eq!(h.record("c", "select 1;\nselect 2", Kind::Edit, 3_000).unwrap(), Recorded::Written);
        let l = h.list("c");
        assert_eq!(l.len(), 2);
        assert_eq!(h.read("c", &l[1].id).unwrap(), "select 1", "the version that ran is still there");
        // Running text that is already a run changes nothing.
        assert_eq!(h.record("c", "select 1;\nselect 2", Kind::Run, 4_000).unwrap(), Recorded::Promoted);
        assert_eq!(h.record("c", "select 1;\nselect 2", Kind::Run, 5_000).unwrap(), Recorded::Skipped);
    }

    #[test]
    fn edits_within_fifteen_seconds_replace_each_other_but_runs_never_do() {
        let (_d, h) = store();
        assert_eq!(h.record("c", "se", Kind::Edit, 1_000).unwrap(), Recorded::Written);
        assert_eq!(h.record("c", "sel", Kind::Edit, 5_000).unwrap(), Recorded::Coalesced);
        assert_eq!(h.record("c", "select", Kind::Edit, 15_999).unwrap(), Recorded::Coalesced);
        let l = h.list("c");
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].at_ms, h.read("c", &l[0].id).unwrap()), (1_000, "select".to_string()));
        // A run is its own snapshot, even of the same burst.
        assert_eq!(h.record("c", "select 1", Kind::Run, 16_000).unwrap(), Recorded::Written);
        // And the next edit starts a fresh one: the newest is a run, not an edit.
        assert_eq!(h.record("c", "select 12", Kind::Edit, 16_500).unwrap(), Recorded::Written);
        assert_eq!(h.list("c").len(), 3);
        // After the window passes a new edit is a new snapshot.
        assert_eq!(h.record("c", "select 123", Kind::Edit, 40_000).unwrap(), Recorded::Written);
        assert_eq!(h.list("c").len(), 4);
    }

    #[test]
    fn the_first_edit_after_a_restart_does_not_overwrite_what_the_last_session_left() {
        let (d, h) = store();
        h.record("c", "left behind by the last session", Kind::Edit, 1_000).unwrap();
        let h2 = LocalHistory::new(d.path().to_path_buf()).session_from(5_000);
        assert_eq!(h2.record("c", "typed after the restart", Kind::Edit, 6_000).unwrap(), Recorded::Written);
        assert_eq!(h2.list("c").len(), 2, "both survive");
        // Within the new session, coalescing works as before.
        assert_eq!(
            h2.record("c", "typed after the restart, more", Kind::Edit, 7_000).unwrap(),
            Recorded::Coalesced
        );
        assert_eq!(h2.list("c").len(), 2);
    }

    #[test]
    fn a_text_over_the_cap_is_not_stored_and_says_so() {
        let (_d, h) = store();
        let big = "x".repeat(MAX_SNAPSHOT_BYTES + 1);
        assert_eq!(h.record("c", &big, Kind::Run, 0).unwrap(), Recorded::TooLarge);
        assert!(h.list("c").is_empty());
        let fits = "x".repeat(MAX_SNAPSHOT_BYTES);
        assert_eq!(h.record("c", &fits, Kind::Run, 0).unwrap(), Recorded::Written);
    }

    #[test]
    fn each_console_is_kept_to_its_count_cap_dropping_the_oldest() {
        let (_d, h) = store();
        for i in 0..(MAX_PER_CONSOLE as u64 + 25) {
            h.record("c", &format!("select {i}"), Kind::Run, 1_000 + i).unwrap();
        }
        let l = h.list("c");
        assert_eq!(l.len(), MAX_PER_CONSOLE);
        assert_eq!(
            h.read("c", &l[0].id).unwrap(),
            format!("select {}", MAX_PER_CONSOLE + 24),
            "the newest survives"
        );
        assert_eq!(h.read("c", &l.last().unwrap().id).unwrap(), "select 25", "the 25 oldest are gone");
    }

    #[test]
    fn the_byte_cap_prunes_but_never_the_newest() {
        let (_d, h) = store();
        let chunk = "y".repeat(900 * 1024);
        for i in 0..8u64 {
            h.record("c", &format!("{i}{chunk}"), Kind::Run, 1_000 + i).unwrap();
        }
        let l = h.list("c");
        let total: u64 = l.iter().map(|m| m.bytes).sum();
        assert!(total <= MAX_CONSOLE_BYTES, "{total}");
        assert!(h.read("c", &l[0].id).unwrap().starts_with('7'), "the newest is kept");
        assert!(l.len() >= 3 && l.len() < 8);
    }

    #[test]
    fn the_total_cap_drops_the_oldest_across_consoles_but_keeps_each_ones_newest() {
        let (_d, h) = store();
        let h = h.with_total_cap(10 * 1024 * 1024);
        let chunk = vec![b'z'; 256 * 1024];
        for (key, base) in [("a", 1_000u64), ("b", 100_000u64)] {
            let dir = h.root().join(key);
            std::fs::create_dir_all(&dir).unwrap();
            for i in 0..40u64 {
                std::fs::write(dir.join(format!("{:013}-run.sql", base + i)), &chunk).unwrap();
            }
        }
        // 20 MiB on disk, over the 10 MiB cap used here.
        h.prune("a");
        let size = |k: &str| h.list(k).iter().map(|m| m.bytes).sum::<u64>();
        assert!(size("a") + size("b") <= 10 * 1024 * 1024);
        assert!(!h.list("a").is_empty() && !h.list("b").is_empty(), "each console keeps its newest");
        assert!(h.list("a").len() < 40, "the older console lost the most");
    }

    #[test]
    fn stray_files_and_hostile_names_are_ignored() {
        let (d, h) = store();
        h.record("c", "select 1", Kind::Run, 1).unwrap();
        std::fs::write(d.path().join("c").join("0000000000002-run.sql.tmp"), "torn").unwrap();
        std::fs::write(d.path().join("c").join("notes.txt"), "x").unwrap();
        assert_eq!(h.list("c").len(), 1);
        assert!(h.read("c", "../../etc/passwd").is_none());
        assert!(h.read("c", "0000000000001-bogus.sql").is_none());
    }

    #[test]
    fn a_key_cannot_escape_the_history_directory() {
        let (d, h) = store();
        h.record("../evil", "select 1", Kind::Run, 1).unwrap();
        assert!(d.path().join("evil").is_dir(), "dots and slashes are dropped from the key");
        assert!(!d.path().parent().unwrap().join("evil").exists());
    }

    #[test]
    fn two_snapshots_in_the_same_millisecond_do_not_overwrite_each_other() {
        let (_d, h) = store();
        h.record("c", "a", Kind::Run, 5).unwrap();
        h.record("c", "b", Kind::Run, 5).unwrap();
        assert_eq!(h.list("c").len(), 2);
    }

    #[test]
    fn summaries_use_the_first_real_line() {
        assert_eq!(summary("\n\n  select * from t  \nwhere x"), "select * from t");
        assert_eq!(summary("   "), "");
        assert!(summary(&"a".repeat(200)).ends_with('…'));
        assert_eq!(summary(&"a".repeat(90)).chars().count(), 90);
    }

    #[test]
    fn removing_a_console_removes_its_history() {
        let (_d, h) = store();
        h.record("c", "select 1", Kind::Run, 1).unwrap();
        h.remove("c");
        assert!(h.list("c").is_empty());
    }
}
