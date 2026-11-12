//! The user's live templates: `<config dir>/templates.json`, re-read when the file changes.
//!
//! The format, the built-ins and the variable substitution are `theta_sql::templates`; this module only
//! owns the file. It is read lazily and at most once a second, so editing the file in an external editor
//! takes effect without a restart and a console that completes on every keystroke does not hit the disk.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use theta_sql::templates::{Template, parse_user_templates, sample_file};

struct State {
    user: Vec<Template>,
    warnings: Vec<String>,
    mtime: Option<SystemTime>,
    checked: Option<Instant>,
}

pub struct TemplateStore {
    path: PathBuf,
    state: Mutex<State>,
}

impl TemplateStore {
    pub fn new(path: PathBuf) -> Arc<TemplateStore> {
        Arc::new(TemplateStore {
            path,
            state: Mutex::new(State { user: Vec::new(), warnings: Vec::new(), mtime: None, checked: None }),
        })
    }

    /// The store for the real config directory.
    pub fn global() -> Arc<TemplateStore> {
        static STORE: OnceLock<Arc<TemplateStore>> = OnceLock::new();
        STORE.get_or_init(|| TemplateStore::new(theta_ui::config_dir().join("templates.json"))).clone()
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn refresh(&self, force: bool) {
        let Ok(mut st) = self.state.lock() else { return };
        if !force && st.checked.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        st.checked = Some(Instant::now());
        let mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if !force && st.mtime == mtime && st.mtime.is_some() {
            return;
        }
        if mtime.is_none() {
            st.user.clear();
            st.warnings.clear();
            st.mtime = None;
            return;
        }
        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                let (user, warnings) = parse_user_templates(&text);
                st.user = user;
                st.warnings = warnings;
            }
            Err(e) => {
                st.user.clear();
                st.warnings = vec![theta_core::friendly::io_message(&e, &self.path, theta_core::friendly::IoOp::Read)];
            }
        }
        st.mtime = mtime;
    }

    /// The user's templates (not the built-ins).
    pub fn user(&self) -> Vec<Template> {
        self.refresh(false);
        self.state.lock().map(|s| s.user.clone()).unwrap_or_default()
    }

    /// Problems found in the file, one line each.
    pub fn warnings(&self) -> Vec<String> {
        self.refresh(false);
        self.state.lock().map(|s| s.warnings.clone()).unwrap_or_default()
    }

    pub fn reload(&self) {
        self.refresh(true);
    }

    /// Create the file with an example if it does not exist, so there is something to edit.
    pub fn ensure_file(&self) -> std::io::Result<()> {
        if self.path.exists() {
            return Ok(());
        }
        theta_ui::settings::write_atomic(&self.path, sample_file().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_file_reports_problems_and_follows_edits() {
        let dir = tempfile::tempdir().unwrap();
        let store = TemplateStore::new(dir.path().join("templates.json"));
        assert!(store.user().is_empty() && store.warnings().is_empty(), "a missing file is no templates");
        std::fs::write(store.path(), r#"[{"abbreviation":"a","body":"x"},{"abbreviation":"","body":"y"}]"#)
            .unwrap();
        store.reload();
        assert_eq!(store.user().len(), 1);
        assert_eq!(store.warnings().len(), 1);
        std::fs::write(store.path(), r#"[{"abbreviation":"a","body":"x"},{"abbreviation":"b","body":"y"}]"#)
            .unwrap();
        store.reload();
        assert_eq!(store.user().len(), 2);
        assert!(store.warnings().is_empty());
        std::fs::remove_file(store.path()).unwrap();
        store.reload();
        assert!(store.user().is_empty());
    }

    #[test]
    fn ensure_file_writes_a_parseable_sample_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = TemplateStore::new(dir.path().join("templates.json"));
        store.ensure_file().unwrap();
        store.reload();
        assert_eq!(store.user().len(), 1);
        std::fs::write(store.path(), "[]").unwrap();
        store.ensure_file().unwrap();
        assert_eq!(
            std::fs::read_to_string(store.path()).unwrap(),
            "[]",
            "an existing file is never overwritten"
        );
    }
}
