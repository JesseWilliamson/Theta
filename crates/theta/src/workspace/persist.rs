//! Panel sizes and visibility, stored in `<config_dir>/workspace.json`.

use super::BottomTab;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_EXPLORER_WIDTH: f32 = 280.0;
pub const DEFAULT_BOTTOM_HEIGHT: f32 = 320.0;
pub const MIN_EXPLORER_WIDTH: f32 = 180.0;
pub const MAX_EXPLORER_WIDTH: f32 = 640.0;
pub const MIN_BOTTOM_HEIGHT: f32 = 120.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceState {
    pub explorer_open: bool,
    pub explorer_width: f32,
    pub bottom_open: bool,
    pub bottom_height: f32,
    pub bottom_tab: BottomTab,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            explorer_open: true,
            explorer_width: DEFAULT_EXPLORER_WIDTH,
            bottom_open: true,
            bottom_height: DEFAULT_BOTTOM_HEIGHT,
            bottom_tab: BottomTab::Results,
        }
    }
}

impl WorkspaceState {
    /// Keep persisted sizes usable even if the file was edited or the screen shrank.
    pub fn sanitized(mut self) -> Self {
        let clamp =
            |v: f32, lo: f32, hi: f32, default: f32| if v.is_finite() { v.clamp(lo, hi) } else { default };
        self.explorer_width =
            clamp(self.explorer_width, MIN_EXPLORER_WIDTH, MAX_EXPLORER_WIDTH, DEFAULT_EXPLORER_WIDTH);
        self.bottom_height = clamp(self.bottom_height, MIN_BOTTOM_HEIGHT, 4000.0, DEFAULT_BOTTOM_HEIGHT);
        self
    }

    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<WorkspaceState>(&t).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, path: &std::path::Path) {
        let json = serde_json::to_string_pretty(self).unwrap_or_default() + "\n";
        // Layout is a convenience: failing to remember it must not interrupt work.
        let _ = theta_ui::settings::write_atomic(path, json.as_bytes());
    }
}

pub fn workspace_path() -> PathBuf {
    theta_ui::config_dir().join("workspace.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        assert_eq!(WorkspaceState::load(&path), WorkspaceState::default());
        let s = WorkspaceState {
            explorer_open: false,
            explorer_width: 333.0,
            bottom_height: 210.0,
            bottom_tab: BottomTab::Plan,
            ..Default::default()
        };
        s.save(&path);
        assert_eq!(WorkspaceState::load(&path), s);
    }

    #[test]
    fn garbage_and_extreme_values_are_tamed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(WorkspaceState::load(&path), WorkspaceState::default());
        std::fs::write(&path, r#"{"explorer_width": 9999, "bottom_height": -5, "unknown": 1}"#).unwrap();
        let s = WorkspaceState::load(&path);
        assert_eq!(s.explorer_width, MAX_EXPLORER_WIDTH);
        assert_eq!(s.bottom_height, MIN_BOTTOM_HEIGHT);
        assert!(s.explorer_open);
    }
}
