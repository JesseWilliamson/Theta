//! Theme registry glue: bundled themes, user themes, applying a theme together
//! with the font settings, and Zed theme import.
//!
//! gpui-kit's own `ThemeRegistry::watch_dir` rebuilds the registry from the
//! watched directory only, which would drop the embedded themes, so user themes
//! are loaded once at start-up and newly imported files are registered
//! directly.

use crate::settings::{Settings, config_dir};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeRegistry};
use gpui_kit::{App, SharedString, px};
use std::path::{Path, PathBuf};

pub const QUARRY_DARK: &str = "Quarry Dark";
pub const QUARRY_LIGHT: &str = "Quarry Light";

pub(crate) const QUARRY_THEMES_JSON: &str = include_str!("../assets/themes/quarry.json");

/// (file name, contents) of every bundled theme file, Quarry's own first.
pub(crate) const BUNDLED: &[(&str, &str)] = &[
    ("quarry.json", QUARRY_THEMES_JSON),
    ("adventure.json", include_str!("../assets/themes/adventure.json")),
    ("alduin.json", include_str!("../assets/themes/alduin.json")),
    ("asciinema.json", include_str!("../assets/themes/asciinema.json")),
    ("aurora.json", include_str!("../assets/themes/aurora.json")),
    ("ayu.json", include_str!("../assets/themes/ayu.json")),
    ("catppuccin.json", include_str!("../assets/themes/catppuccin.json")),
    ("everforest.json", include_str!("../assets/themes/everforest.json")),
    ("fahrenheit.json", include_str!("../assets/themes/fahrenheit.json")),
    ("flexoki.json", include_str!("../assets/themes/flexoki.json")),
    ("gruvbox.json", include_str!("../assets/themes/gruvbox.json")),
    ("harper.json", include_str!("../assets/themes/harper.json")),
    ("hybrid.json", include_str!("../assets/themes/hybrid.json")),
    ("jellybeans.json", include_str!("../assets/themes/jellybeans.json")),
    ("kibble.json", include_str!("../assets/themes/kibble.json")),
    ("macos-classic.json", include_str!("../assets/themes/macos-classic.json")),
    ("mellifluous.json", include_str!("../assets/themes/mellifluous.json")),
    ("molokai.json", include_str!("../assets/themes/molokai.json")),
    ("solarized.json", include_str!("../assets/themes/solarized.json")),
    ("spaceduck.json", include_str!("../assets/themes/spaceduck.json")),
    ("tokyonight.json", include_str!("../assets/themes/tokyonight.json")),
    ("twilight.json", include_str!("../assets/themes/twilight.json")),
];

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeInfo {
    pub name: SharedString,
    pub dark: bool,
}

pub(crate) fn register_bundled(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for (file, json) in BUNDLED {
        if let Err(e) = registry.load_themes_from_str(json) {
            eprintln!("quarry-ui: bundled theme {file} is invalid: {e}");
        }
    }
}

/// Register every `*.json` in `<config_dir>/themes/`. Returns one warning per
/// unreadable or invalid file.
pub(crate) fn load_user_themes(cx: &mut App) -> Vec<String> {
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(themes_dir()) else { return warnings };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    for path in files {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(&text) {
                    warnings.push(format!("theme {}: {e}", path.display()));
                }
            }
            Err(e) => warnings.push(format!("theme {}: {e}", path.display())),
        }
    }
    warnings
}

/// Every registered theme: Quarry's first, then light before dark by name.
pub fn available(cx: &App) -> Vec<ThemeInfo> {
    let mut list: Vec<ThemeInfo> = ThemeRegistry::global(cx)
        .themes()
        .values()
        .map(|t| ThemeInfo { name: t.name.clone(), dark: t.mode.is_dark() })
        .collect();
    list.sort_by_key(|t| {
        let rank = match t.name.as_ref() {
            QUARRY_DARK => 0,
            QUARRY_LIGHT => 1,
            _ => 2,
        };
        (rank, t.dark, t.name.to_lowercase())
    });
    list
}

pub fn current_name(cx: &App) -> SharedString {
    cx.theme().theme_name().clone()
}

/// The theme name the settings ask for right now, honouring "follow system".
pub fn desired_name(cx: &App) -> String {
    let s = Settings::get(cx);
    if s.follow_system_appearance {
        match cx.window_appearance() {
            gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight => {
                s.light_theme.clone()
            }
            _ => s.dark_theme.clone(),
        }
    } else {
        s.theme.clone()
    }
}

/// Apply a theme by name without touching the saved settings (used for live
/// preview). Returns false when no such theme is registered.
pub fn preview(name: &str, cx: &mut App) -> bool {
    let Some(config) = ThemeRegistry::global(cx).themes().get(name).cloned() else {
        return false;
    };
    let (ui_family, ui_size, mono_family, mono_size) = {
        let s = Settings::get(cx);
        (s.ui_font_family.clone(), s.ui_font_size, s.buffer_font_family.clone(), s.buffer_font_size)
    };
    Theme::update(cx, |theme| {
        theme.apply_config(&config);
        // Fonts are a user setting, not part of a colour theme, so they win
        // over whatever a theme file might say.
        theme.font_family = ui_family.into();
        theme.font_size = px(ui_size);
        theme.mono_font_family = mono_family.into();
        theme.mono_font_size = px(mono_size);
    });
    true
}

/// (Re)apply whatever the settings say; falls back to the mode defaults when
/// the saved theme no longer exists.
pub fn apply_from_settings(cx: &mut App) {
    let name = desired_name(cx);
    if !preview(&name, cx) {
        let fallback = if matches!(
            cx.window_appearance(),
            gpui_kit::WindowAppearance::Light | gpui_kit::WindowAppearance::VibrantLight
        ) {
            QUARRY_LIGHT
        } else {
            QUARRY_DARK
        };
        preview(fallback, cx);
    }
}

/// Persist `name` as the active theme and apply it immediately (observers
/// only run after the current update cycle, which is too late during start-up).
pub fn set_theme(name: &str, cx: &mut App) {
    let name = name.to_string();
    Settings::update(cx, |s| s.theme = name.clone());
    preview(&name, cx);
}

/// Convert a Zed theme file, store it in `<config_dir>/themes/` and register
/// it. Returns the names of the imported themes.
pub fn import_zed_theme_file(src: &Path, cx: &mut App) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(src)?;
    let converted = crate::zed_import::import_zed_theme(&text)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("imported");
    let dest = themes_dir().join(format!("{}.json", crate::zed_import::slug(stem)));
    crate::settings::write_atomic(&dest, converted.as_bytes())?;
    let names = crate::zed_import::theme_names(&converted);
    ThemeRegistry::global_mut(cx).load_themes_from_str(&converted)?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_file_parses_as_a_theme_set() {
        for (file, json) in BUNDLED {
            let set: gpui_kit::component::ThemeSet =
                serde_json::from_str(json).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(!set.themes.is_empty(), "{file} has no themes");
        }
        assert_eq!(BUNDLED.len(), 22, "21 gpui-kit theme files plus quarry.json");
    }

    #[test]
    fn quarry_themes_have_the_reference_palette() {
        let set: gpui_kit::component::ThemeSet = serde_json::from_str(QUARRY_THEMES_JSON).unwrap();
        let names: Vec<_> = set.themes.iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, vec![QUARRY_DARK, QUARRY_LIGHT]);
        let dark = &set.themes[0];
        assert!(dark.mode.is_dark());
        assert_eq!(dark.colors.background.as_ref().unwrap().as_ref(), "#282c33");
        assert_eq!(dark.colors.border.as_ref().unwrap().as_ref(), "#464b57");
        let light = &set.themes[1];
        assert!(!light.mode.is_dark());
        assert_eq!(light.colors.background.as_ref().unwrap().as_ref(), "#fafafa");
    }
}
