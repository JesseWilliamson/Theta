//! User settings: one JSON file, tolerant on read, persisted on every change.
//!
//! The file is hand-editable (the command palette has "Open Settings File"), so
//! reading must never be fatal: a missing key takes its default, an unknown key
//! is ignored, and a key with a wrong type keeps its default and produces a
//! warning the shell can show in a toast instead of crashing or silently
//! discarding the rest of the file.

use gpui_kit::{App, Global, Subscription};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which editing keymap the SQL editor uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeymapMode {
    #[default]
    Helix,
    Vim,
    Standard,
}

impl KeymapMode {
    pub const ALL: [KeymapMode; 3] = [KeymapMode::Helix, KeymapMode::Vim, KeymapMode::Standard];

    pub fn label(self) -> &'static str {
        match self {
            KeymapMode::Helix => "Helix",
            KeymapMode::Vim => "Vim",
            KeymapMode::Standard => "Standard",
        }
    }

    /// Whether this keymap is modal (Helix, Vim). Panels that offer "vim-style" navigation
    /// (h j k l, gg/G ...) check this rather than a specific keymap, because every modal user
    /// expects those keys outside the editor too.
    pub fn is_modal(self) -> bool {
        matches!(self, KeymapMode::Helix | KeymapMode::Vim)
    }
}

/// Settings of the Vim keymap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VimSettings {
    /// Like `set clipboard=unnamedplus`: yanks and deletes go to the system clipboard and `p`
    /// pastes what other applications copied.
    pub clipboard_unnamed: bool,
}

impl Default for VimSettings {
    fn default() -> Self {
        Self { clipboard_unnamed: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormatSettings {
    pub indent: usize,
    pub uppercase_keywords: bool,
}

impl Default for FormatSettings {
    fn default() -> Self {
        Self { indent: 4, uppercase_keywords: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub follow_system_appearance: bool,
    pub light_theme: String,
    pub dark_theme: String,
    pub keymap: KeymapMode,
    pub vim: VimSettings,
    pub ui_font_family: String,
    pub ui_font_size: f32,
    pub buffer_font_family: String,
    pub buffer_font_size: f32,
    /// Multiplier of the buffer font size.
    pub buffer_line_height: f32,
    pub row_limit: usize,
    /// XYZ tile URL template for the map view.
    /// Legacy single tile URL; `sanitized` migrates a changed value into a custom basemap
    /// and puts the default back.
    pub map_tile_url: String,
    /// Id of the selected basemap: a built-in (`osm`, `opentopomap`, `none`) or a custom id.
    pub map_basemap: String,
    /// User-defined basemaps.
    pub map_custom_basemaps: Vec<CustomBasemap>,
    pub format: FormatSettings,
}

pub const DEFAULT_UI_FONT: &str = "IBM Plex Sans";
pub const DEFAULT_BUFFER_FONT: &str = "Lilex";
/// A basemap the user defined in settings.json or Settings > Map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomBasemap {
    pub id: String,
    pub name: String,
    /// XYZ template with `{z}`, `{x}`, `{y}` and optionally `{s}` and `{r}`.
    pub url: String,
    pub attribution: String,
    /// Deepest zoom level the tiles exist at; the map scales them beyond it.
    pub max_zoom: u8,
    /// A dark basemap is not dimmed in dark themes.
    pub dark: bool,
}

impl Default for CustomBasemap {
    fn default() -> Self {
        CustomBasemap {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            attribution: String::new(),
            max_zoom: 19,
            dark: false,
        }
    }
}

impl CustomBasemap {
    /// Whether `url` can be used as an XYZ tile template.
    pub fn url_is_valid(url: &str) -> bool {
        (url.starts_with("http://") || url.starts_with("https://"))
            && ["{z}", "{x}", "{y}"].iter().all(|p| url.contains(p))
    }
}

pub const DEFAULT_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "Quarry Dark".into(),
            follow_system_appearance: false,
            light_theme: "Quarry Light".into(),
            dark_theme: "Quarry Dark".into(),
            keymap: KeymapMode::Helix,
            vim: VimSettings::default(),
            ui_font_family: DEFAULT_UI_FONT.into(),
            ui_font_size: 14.0,
            buffer_font_family: DEFAULT_BUFFER_FONT.into(),
            buffer_font_size: 14.0,
            buffer_line_height: 1.5,
            row_limit: 1000,
            map_tile_url: DEFAULT_TILE_URL.into(),
            map_basemap: "osm".into(),
            map_custom_basemaps: Vec::new(),
            format: FormatSettings::default(),
        }
    }
}

pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 40.0;

impl Settings {
    /// Parse a settings file leniently. Returns the settings plus one warning
    /// per ignored key, or a single warning when the text is not JSON at all.
    pub fn parse_lenient(text: &str) -> (Settings, Vec<String>) {
        let mut warnings = Vec::new();
        let mut current = serde_json::to_value(Settings::default()).expect("defaults serialize");
        if text.trim().is_empty() {
            return (Settings::default(), warnings);
        }
        let parsed: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                warnings.push(format!("settings.json is not valid JSON ({e}); using defaults"));
                return (Settings::default(), warnings);
            }
        };
        let Some(object) = parsed.as_object() else {
            warnings.push("settings.json must contain a JSON object; using defaults".into());
            return (Settings::default(), warnings);
        };
        // Validate key by key: a candidate that fails to deserialize blames
        // exactly the key just merged, so one typo does not reset everything.
        for (key, value) in object {
            if !current.as_object().is_some_and(|o| o.contains_key(key)) {
                continue; // unknown keys are tolerated (future versions, comments)
            }
            let mut candidate = current.clone();
            candidate[key] = value.clone();
            match serde_json::from_value::<Settings>(candidate.clone()) {
                Ok(_) => current = candidate,
                Err(e) => warnings.push(format!("settings.json: ignoring \"{key}\" ({e})")),
            }
        }
        let settings: Settings = serde_json::from_value(current).unwrap_or_default();
        (settings.sanitized(), warnings)
    }

    fn sanitized(mut self) -> Self {
        self.ui_font_size = self.ui_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.buffer_font_size = self.buffer_font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.buffer_line_height = self.buffer_line_height.clamp(1.0, 3.0);
        self.row_limit = self.row_limit.max(1);
        // The old single-URL setting becomes a custom basemap the first time it is seen changed.
        if self.map_tile_url != DEFAULT_TILE_URL {
            if CustomBasemap::url_is_valid(&self.map_tile_url)
                && !self.map_custom_basemaps.iter().any(|b| b.url == self.map_tile_url)
            {
                self.map_custom_basemaps.push(CustomBasemap {
                    id: "custom-migrated".into(),
                    name: "Custom tiles".into(),
                    url: self.map_tile_url.clone(),
                    ..CustomBasemap::default()
                });
                if self.map_basemap == "osm" {
                    self.map_basemap = "custom-migrated".into();
                }
            }
            self.map_tile_url = DEFAULT_TILE_URL.into();
        }
        let mut seen = std::collections::HashSet::new();
        self.map_custom_basemaps.retain_mut(|b| {
            b.max_zoom = b.max_zoom.clamp(1, 22);
            if b.id.is_empty() {
                b.id = format!("custom-{}", b.name.to_lowercase().replace(' ', "-"));
            }
            !b.name.trim().is_empty() && CustomBasemap::url_is_valid(&b.url) && seen.insert(b.id.clone())
        });
        self
    }

    pub fn to_pretty_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("settings serialize") + "\n"
    }

    pub fn load(path: &Path) -> (Settings, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse_lenient(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), vec![]),
            Err(e) => (Settings::default(), vec![format!("could not read {}: {e}", path.display())]),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        write_atomic(path, self.to_pretty_json().as_bytes())
    }
}

/// Write via a temp file so a crash mid-write cannot leave a truncated file
/// that the next start would reject.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// `QUARRY_CONFIG_DIR` when set, otherwise `<OS config dir>/quarry`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("QUARRY_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("quarry")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

/// The GPUI global that owns the live settings.
pub struct SettingsStore {
    settings: Settings,
    path: PathBuf,
    warnings: Vec<String>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub(crate) fn push_warnings(&mut self, warnings: Vec<String>) {
        self.warnings.extend(warnings);
    }

    pub(crate) fn install(cx: &mut App) {
        let path = settings_path();
        let (settings, warnings) = Settings::load(&path);
        cx.set_global(SettingsStore { settings, path, warnings });
    }
}

impl Settings {
    pub fn get(cx: &App) -> &Settings {
        &cx.global::<SettingsStore>().settings
    }

    /// Apply `edit`, persist when something changed and notify observers.
    /// A failed write is reported through [`Settings::warnings`], not swallowed.
    pub fn update(cx: &mut App, edit: impl FnOnce(&mut Settings)) {
        let store = cx.global::<SettingsStore>();
        let mut next = store.settings.clone();
        edit(&mut next);
        let next = next.sanitized();
        if next == store.settings {
            return;
        }
        // `global_mut` (unlike `update_global`) marks the global as changed, which
        // is what wakes `observe_global` subscribers.
        let store = cx.global_mut::<SettingsStore>();
        store.settings = next;
        if let Err(e) = store.settings.save(&store.path) {
            store.warnings.push(format!("could not save {}: {e}", store.path.display()));
        }
    }

    /// Problems found while loading or saving; the shell shows them once.
    pub fn warnings(cx: &App) -> Vec<String> {
        cx.global::<SettingsStore>().warnings.clone()
    }

    /// Report a problem the shell shows once (a theme that failed to load, a failed save).
    pub fn warn(cx: &mut App, message: impl Into<String>) {
        cx.global_mut::<SettingsStore>().warnings.push(message.into());
    }

    pub fn take_warnings(cx: &mut App) -> Vec<String> {
        std::mem::take(&mut cx.global_mut::<SettingsStore>().warnings)
    }

    pub fn path(cx: &App) -> PathBuf {
        cx.global::<SettingsStore>().path.clone()
    }

    /// Run `f` whenever settings change.
    pub fn observe(cx: &mut App, mut f: impl FnMut(&mut App) + 'static) -> Subscription {
        cx.observe_global::<SettingsStore>(move |cx| f(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let s = Settings::default();
        let (back, warnings) = Settings::parse_lenient(&s.to_pretty_json());
        assert_eq!(back, s);
        assert!(warnings.is_empty());
    }

    #[test]
    fn missing_and_unknown_keys_are_tolerated() {
        let (s, w) = Settings::parse_lenient(r#"{ "theme": "Tokyo Night", "future_option": [1,2] }"#);
        assert_eq!(s.theme, "Tokyo Night");
        assert_eq!(s.row_limit, 1000);
        assert!(w.is_empty());
    }

    #[test]
    fn wrong_type_only_resets_that_key() {
        let (s, w) =
            Settings::parse_lenient(r#"{ "keymap": "emacs", "row_limit": 50, "ui_font_size": "big" }"#);
        assert_eq!(s.keymap, KeymapMode::Helix);
        assert_eq!(s.row_limit, 50);
        assert_eq!(s.ui_font_size, 14.0);
        assert_eq!(w.len(), 2, "{w:?}");
    }

    #[test]
    fn malformed_file_keeps_defaults_and_warns() {
        let (s, w) = Settings::parse_lenient("{ not json");
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
        let (s, w) = Settings::parse_lenient("[1]");
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn a_changed_legacy_tile_url_becomes_a_selected_custom_basemap() {
        let (s, w) =
            Settings::parse_lenient(r#"{ "map_tile_url": "https://tiles.example/{z}/{x}/{y}.png" }"#);
        assert!(w.is_empty());
        assert_eq!(s.map_custom_basemaps.len(), 1);
        assert_eq!(s.map_custom_basemaps[0].url, "https://tiles.example/{z}/{x}/{y}.png");
        assert_eq!(s.map_basemap, "custom-migrated");
        assert_eq!(s.map_tile_url, DEFAULT_TILE_URL, "migrated exactly once");
        // The default URL migrates nothing, and a re-save does not duplicate the entry.
        let (d, _) = Settings::parse_lenient("{}");
        assert!(d.map_custom_basemaps.is_empty() && d.map_basemap == "osm");
        let (again, _) = Settings::parse_lenient(&s.to_pretty_json());
        assert_eq!(again.map_custom_basemaps.len(), 1);
    }

    #[test]
    fn invalid_custom_basemaps_are_dropped() {
        let (s, _) = Settings::parse_lenient(
            r#"{ "map_custom_basemaps": [
                {"name": "ok", "url": "https://t/{z}/{x}/{y}.png", "max_zoom": 99},
                {"name": "no placeholders", "url": "https://t/tile.png"},
                {"name": "", "url": "https://t/{z}/{x}/{y}.png"},
                {"name": "file", "url": "file:///{z}/{x}/{y}"} ] }"#,
        );
        assert_eq!(s.map_custom_basemaps.len(), 1);
        assert_eq!(s.map_custom_basemaps[0].max_zoom, 22);
    }

    #[test]
    fn vim_keymap_and_clipboard_setting_round_trip() {
        let (s, w) = Settings::parse_lenient(r#"{ "keymap": "vim", "vim": { "clipboard_unnamed": false } }"#);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.keymap, KeymapMode::Vim);
        assert!(!s.vim.clipboard_unnamed);
        assert!(Settings::default().vim.clipboard_unnamed, "on by default, like unnamedplus");
        assert!(s.to_pretty_json().contains("\"keymap\": \"vim\""));
        assert!(
            KeymapMode::Vim.is_modal() && KeymapMode::Helix.is_modal() && !KeymapMode::Standard.is_modal()
        );
    }

    #[test]
    fn font_sizes_are_clamped() {
        let (s, _) = Settings::parse_lenient(r#"{ "ui_font_size": 400, "buffer_font_size": 1 }"#);
        assert_eq!(s.ui_font_size, MAX_FONT_SIZE);
        assert_eq!(s.buffer_font_size, MIN_FONT_SIZE);
    }

    #[test]
    fn save_and_load_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        let s = Settings {
            keymap: KeymapMode::Standard,
            format: FormatSettings { indent: 2, ..Default::default() },
            ..Default::default()
        };
        s.save(&path).unwrap();
        let (loaded, w) = Settings::load(&path);
        assert_eq!(loaded, s);
        assert!(w.is_empty());
        let (missing, w) = Settings::load(&dir.path().join("none.json"));
        assert_eq!(missing, Settings::default());
        assert!(w.is_empty());
    }
}
