//! # quarry-ui
//!
//! The app-agnostic UI foundation every Quarry view crate builds on: settings,
//! bundled fonts and themes, colour accessors for custom-painted views, shared
//! density tokens and the small set of primitives (panel header, empty state,
//! key-binding chip, picker) that give all panels the same look.
//!
//! This crate depends on gpui-kit and nothing else in the workspace, so the
//! editor, plan and map crates can use it without pulling in the app.

mod assets;
pub mod colors;
mod db_icon;
mod fonts;
pub mod fuzzy;
mod keyhint;
mod panel;
pub mod picker;
pub mod settings;
pub mod themes;
pub mod tokens;
pub mod zed_import;

pub use assets::QuarryAssets;
pub use colors::{EditorColors, SyntaxColors};
pub use db_icon::{DbIcon, DbKindIcon};
pub use keyhint::{KeyHint, format_keystrokes, is_mac};
pub use panel::{EmptyState, PanelHeader, tool_button};
pub use settings::{KeymapMode, Settings, config_dir};
pub use themes::{ThemeInfo, import_zed_theme_file};
pub use tokens::*;
pub use zed_import::import_zed_theme;

use gpui_kit::App;

/// A gpui-kit application with Quarry's combined asset source installed.
pub fn application() -> gpui_kit::Application {
    gpui_kit::application().with_assets(QuarryAssets)
}

/// Initialise gpui-kit, fonts, themes and settings, then apply the saved theme.
/// Call once at start-up, inside `application().run`.
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    settings::SettingsStore::install(cx);
    fonts::register(cx);
    picker::bind_keys(cx);
    themes::register_bundled(cx);
    let warnings = themes::load_user_themes(cx);
    if !warnings.is_empty() {
        cx.global_mut::<settings::SettingsStore>().push_warnings(warnings);
    }
    themes::apply_from_settings(cx);
    // Re-apply whenever settings change (theme, fonts, follow-system).
    let mut last = Settings::get(cx).clone();
    Settings::observe(cx, move |cx| {
        let now = Settings::get(cx).clone();
        let theme_relevant = now.theme != last.theme
            || now.follow_system_appearance != last.follow_system_appearance
            || now.light_theme != last.light_theme
            || now.dark_theme != last.dark_theme
            || now.ui_font_family != last.ui_font_family
            || now.ui_font_size != last.ui_font_size
            || now.buffer_font_family != last.buffer_font_family
            || now.buffer_font_size != last.buffer_font_size;
        last = now;
        if theme_relevant {
            themes::apply_from_settings(cx);
        }
    })
    .detach();
}

/// Parse `"1440x900"` (as in `QUARRY_WINDOW_SIZE`).
pub fn parse_window_size(text: &str) -> Option<(f32, f32)> {
    let (w, h) = text.trim().split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
    (w >= 200. && h >= 150.).then_some((w, h))
}

/// `QUARRY_WINDOW_SIZE=WxH`, when set and valid.
pub fn window_size_from_env() -> Option<(f32, f32)> {
    std::env::var("QUARRY_WINDOW_SIZE").ok().and_then(|v| parse_window_size(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_size_parsing() {
        assert_eq!(parse_window_size("1440x900"), Some((1440., 900.)));
        assert_eq!(parse_window_size(" 900X600 "), Some((900., 600.)));
        assert_eq!(parse_window_size("10x10"), None);
        assert_eq!(parse_window_size("wide"), None);
    }
}
