//! Convert a Zed theme family (`https://zed.dev/schema/themes/v0.2.0.json`)
//! into gpui-kit's theme JSON.
//!
//! Zed describes dozens of UI surfaces that Quarry does not have and omits
//! several that gpui-kit wants (hover/active variants of every button kind).
//! The converter therefore starts from the bundled Quarry theme of the same
//! appearance and overrides only what the Zed theme states, so the result is
//! always a complete theme and a sparse Zed file still imports sensibly.

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};

/// `(r, g, b, a)`, 0-255.
type Rgba8 = (u8, u8, u8, u8);

fn parse_hex(s: &str) -> Option<Rgba8> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some((byte(0)?, byte(2)?, byte(4)?, 255)),
        8 => Some((byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        3 => {
            let n = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok().map(|v| v * 17);
            Some((n(0)?, n(1)?, n(2)?, 255))
        }
        _ => None,
    }
}

fn to_hex((r, g, b, a): Rgba8) -> String {
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

fn luminance((r, g, b, _): Rgba8) -> f32 {
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.03928 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

fn mix(a: Rgba8, b: Rgba8, t: f32) -> Rgba8 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2), a.3)
}

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "theme".into() } else { out }
}

/// Names of the themes inside an already converted file.
pub fn theme_names(json: &str) -> Vec<String> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| {
            v.get("themes")?
                .as_array()
                .map(|a| a.iter().filter_map(|t| t.get("name")?.as_str().map(str::to_string)).collect())
        })
        .unwrap_or_default()
}

fn base_theme(dark: bool) -> Value {
    let set: Value = serde_json::from_str(crate::themes::QUARRY_THEMES_JSON).expect("bundled theme parses");
    let want = if dark { "dark" } else { "light" };
    set["themes"]
        .as_array()
        .and_then(|a| a.iter().find(|t| t["mode"] == want))
        .cloned()
        .expect("bundled themes include both appearances")
}

/// Convert a Zed theme family JSON document into our theme-set JSON.
pub fn import_zed_theme(json: &str) -> Result<String> {
    let doc: Value = serde_json::from_str(json).context("not valid JSON")?;
    let family = doc.get("name").and_then(Value::as_str).unwrap_or("Imported").to_string();
    let themes = doc
        .get("themes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("not a Zed theme file: missing \"themes\" array"))?;
    if themes.is_empty() {
        bail!("the theme family contains no themes");
    }
    let mut out = Vec::new();
    for t in themes {
        out.push(convert_theme(t)?);
    }
    let mut set = Map::new();
    set.insert("name".into(), json!(family));
    if let Some(author) = doc.get("author").and_then(Value::as_str) {
        set.insert("author".into(), json!(author));
    }
    set.insert("themes".into(), Value::Array(out));
    Ok(serde_json::to_string_pretty(&Value::Object(set))? + "\n")
}

fn convert_theme(t: &Value) -> Result<Value> {
    let name = t.get("name").and_then(Value::as_str).ok_or_else(|| anyhow!("a theme has no name"))?;
    let dark = t.get("appearance").and_then(Value::as_str) != Some("light");
    let style =
        t.get("style").and_then(Value::as_object).ok_or_else(|| anyhow!("theme \"{name}\" has no style"))?;

    let mut theme = base_theme(dark);
    theme["name"] = json!(name);
    theme["is_default"] = json!(false);
    let mut colors = theme["colors"].as_object().cloned().unwrap_or_default();
    let mut hl = theme["highlight"].as_object().cloned().unwrap_or_default();

    let get = |k: &str| -> Option<String> {
        style.get(k).and_then(Value::as_str).filter(|s| parse_hex(s).is_some()).map(str::to_string)
    };
    let mut set = |keys: &[&str], value: &Option<String>| {
        if let Some(v) = value {
            for k in keys {
                colors.insert((*k).into(), json!(v));
            }
        }
    };

    let editor_bg = get("editor.background");
    set(&["background", "table.background", "table.even.background"], &editor_bg);
    set(
        &[
            "foreground",
            "popover.foreground",
            "sidebar.foreground",
            "accent.foreground",
            "secondary.foreground",
            "button.foreground",
            "group_box.foreground",
        ],
        &get("text"),
    );
    set(&["border", "input.border", "window.border", "sidebar.border"], &get("border"));
    set(&["table.row.border", "chart.grid"], &get("border.variant"));
    set(
        &[
            "muted.foreground",
            "tab.foreground",
            "table.head.foreground",
            "table.foot.foreground",
            "description_list_label.foreground",
        ],
        &get("text.muted"),
    );
    let panel = get("panel.background").or_else(|| get("surface.background"));
    set(
        &[
            "sidebar.background",
            "list.background",
            "list.head.background",
            "list.even.background",
            "muted.background",
            "table.head.background",
            "table.foot.background",
            "accordion.background",
            "group_box.background",
            "description_list_label.background",
        ],
        &panel,
    );
    set(&["popover.background"], &get("elevated_surface.background").or(panel.clone()));
    set(&["title_bar.background"], &get("title_bar.background"));
    set(&["status_bar.background"], &get("status_bar.background"));
    set(&["tab_bar.background", "tab_bar.segmented.background"], &get("tab_bar.background"));
    set(&["tab.active.background"], &get("tab.active_background").or(editor_bg.clone()));
    set(&["title_bar.border", "status_bar.border"], &get("border"));
    set(
        &[
            "accent.background",
            "list.hover.background",
            "table.hover.background",
            "sidebar.accent.background",
            "secondary.hover.background",
            "button.hover.background",
        ],
        &get("element.hover"),
    );
    set(
        &[
            "list.active.background",
            "table.active.background",
            "secondary.active.background",
            "button.active.background",
        ],
        &get("element.selected").or_else(|| get("element.active")),
    );
    set(&["button.background", "secondary.background"], &get("element.background"));
    set(&["scrollbar.thumb.background"], &get("scrollbar.thumb.background"));
    set(&["scrollbar.thumb.hover.background"], &get("scrollbar.thumb.hover_background"));

    // Status colours: the solid colour drives gpui-kit's buttons and alerts.
    set(&["danger.background", "chart.bearish", "base.red"], &get("error"));
    set(&["warning.background", "base.yellow"], &get("warning"));
    set(&["success.background", "chart.bullish", "base.green"], &get("success"));
    set(&["info.background", "base.blue"], &get("info"));

    if let Some(accent) = get("text.accent") {
        set(
            &[
                "ring",
                "link.foreground",
                "link.hover.foreground",
                "link.active.foreground",
                "primary.background",
                "list.active.border",
                "table.active.border",
                "drag_border",
                "slider.bar.background",
                "progress_bar.background",
                "caret",
            ],
            &Some(accent.clone()),
        );
        if let Some(a) = parse_hex(&accent) {
            let on = if luminance(a) > 0.35 { "#1b1f27" } else { "#ffffff" };
            for k in [
                "primary.foreground",
                "danger.foreground",
                "warning.foreground",
                "success.foreground",
                "info.foreground",
            ] {
                colors.insert(k.into(), json!(on));
            }
            let toward = if luminance(a) > 0.35 { (0, 0, 0, 255) } else { (255, 255, 255, 255) };
            colors.insert("primary.hover.background".into(), json!(to_hex(mix(a, toward, 0.12))));
            colors.insert("primary.active.background".into(), json!(to_hex(mix(a, (0, 0, 0, 255), 0.15))));
        }
    }

    if let Some(player) = style.get("players").and_then(Value::as_array).and_then(|p| p.first()) {
        if let Some(sel) = player.get("selection").and_then(Value::as_str).filter(|s| parse_hex(s).is_some())
        {
            colors.insert("selection.background".into(), json!(sel));
            colors.insert("drop_target.background".into(), json!(sel));
        }
        if let Some(cursor) = player.get("cursor").and_then(Value::as_str).filter(|s| parse_hex(s).is_some())
        {
            colors.insert("caret".into(), json!(cursor));
        }
    }

    // Editor/highlight section.
    let mut seth = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            hl.insert(k.into(), json!(v));
        }
    };
    seth("editor.background", editor_bg);
    seth("editor.foreground", get("editor.foreground").or_else(|| get("text")));
    seth("editor.active_line.background", get("editor.active_line.background"));
    seth("editor.line_number", get("editor.line_number"));
    seth("editor.active_line_number", get("editor.active_line_number"));
    seth("editor.invisible", get("editor.invisible"));
    seth("editor.gutter.background", get("editor.gutter.background").or_else(|| get("editor.background")));
    for status in ["error", "warning", "success", "info", "hint", "created", "modified", "deleted"] {
        seth(status, get(status));
        for part in ["background", "border"] {
            let key = format!("{status}.{part}");
            seth(&key, get(&key));
        }
    }

    if let Some(syntax) = style.get("syntax").and_then(Value::as_object) {
        let mut out = hl.get("syntax").and_then(Value::as_object).cloned().unwrap_or_default();
        for (key, spec) in syntax {
            let Some(color) = spec.get("color").and_then(Value::as_str).filter(|c| parse_hex(c).is_some())
            else {
                continue;
            };
            let mut entry = Map::new();
            entry.insert("color".into(), json!(color));
            if spec.get("font_style").and_then(Value::as_str) == Some("italic") {
                entry.insert("font_style".into(), json!("italic"));
            }
            if let Some(w) = spec.get("font_weight").and_then(Value::as_u64) {
                entry.insert("font_weight".into(), json!(w));
            }
            out.insert(key.clone(), Value::Object(entry));
        }
        hl.insert("syntax".into(), Value::Object(out));
    }

    theme["colors"] = Value::Object(colors);
    theme["highlight"] = Value::Object(hl);
    Ok(theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like Zed's One Dark: flat dotted keys, `players`, `syntax`.
    const FIXTURE: &str = r##"{
      "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
      "name": "One",
      "author": "Zed Industries",
      "themes": [
        {
          "name": "One Dark Test",
          "appearance": "dark",
          "style": {
            "border": "#464b57ff",
            "border.variant": "#363c46ff",
            "background": "#3b414dff",
            "elevated_surface.background": "#2f343eff",
            "element.background": "#2e343eff",
            "element.hover": "#363c46ff",
            "element.active": "#454a56ff",
            "element.selected": "#454a56ff",
            "text": "#dce0e5ff",
            "text.muted": "#a9afbcff",
            "text.placeholder": "#878a98ff",
            "text.accent": "#74ade8ff",
            "status_bar.background": "#3b414dff",
            "title_bar.background": "#3b414dff",
            "toolbar.background": "#282c33ff",
            "tab_bar.background": "#2f343eff",
            "tab.inactive_background": "#2f343eff",
            "tab.active_background": "#282c33ff",
            "scrollbar.thumb.background": "#a9afbc4c",
            "editor.foreground": "#acb2beff",
            "editor.background": "#282c33ff",
            "editor.gutter.background": "#282c33ff",
            "editor.active_line.background": "#2f343ebf",
            "editor.line_number": "#4e5a5fff",
            "editor.active_line_number": "#d0d4daff",
            "panel.background": "#2f343eff",
            "error": "#d07277ff",
            "error.background": "#d072771a",
            "warning": "#dec184ff",
            "success": "#a1c181ff",
            "info": "#74ade8ff",
            "hint": "#788ca6ff",
            "players": [
              { "cursor": "#74ade8ff", "background": "#74ade8ff", "selection": "#74ade83d" },
              { "cursor": "#be5046ff", "background": "#be5046ff", "selection": "#be50463d" }
            ],
            "syntax": {
              "keyword": { "color": "#b477cfff", "font_style": null, "font_weight": null },
              "string": { "color": "#a1c181ff" },
              "comment": { "color": "#5d636fff", "font_style": "italic" },
              "title": { "color": "#d07277ff", "font_weight": 400 },
              "number": { "color": "#bf956aff" },
              "function": { "color": "#73ade9ff" },
              "type": { "color": "#6eb4bfff" },
              "variable": { "color": "#acb2beff" },
              "broken": { "font_style": "italic" }
            }
          }
        },
        {
          "name": "One Light Test",
          "appearance": "light",
          "style": {
            "text": "#242529ff",
            "text.accent": "#5c78e2ff",
            "editor.background": "#fafafaff",
            "syntax": { "keyword": { "color": "#a449abff" } }
          }
        }
      ]
    }"##;

    fn converted() -> Value {
        serde_json::from_str(&import_zed_theme(FIXTURE).unwrap()).unwrap()
    }

    #[test]
    fn family_and_theme_names_survive() {
        let v = converted();
        assert_eq!(v["name"], "One");
        assert_eq!(v["author"], "Zed Industries");
        assert_eq!(theme_names(&import_zed_theme(FIXTURE).unwrap()), vec!["One Dark Test", "One Light Test"]);
        assert_eq!(v["themes"][0]["mode"], "dark");
        assert_eq!(v["themes"][1]["mode"], "light");
    }

    #[test]
    fn ui_colours_are_mapped() {
        let c = &converted()["themes"][0]["colors"];
        assert_eq!(c["background"], "#282c33ff");
        assert_eq!(c["foreground"], "#dce0e5ff");
        assert_eq!(c["border"], "#464b57ff");
        assert_eq!(c["sidebar.background"], "#2f343eff");
        assert_eq!(c["title_bar.background"], "#3b414dff");
        assert_eq!(c["tab.active.background"], "#282c33ff");
        assert_eq!(c["muted.foreground"], "#a9afbcff");
        assert_eq!(c["list.hover.background"], "#363c46ff");
        assert_eq!(c["list.active.background"], "#454a56ff");
        assert_eq!(c["primary.background"], "#74ade8ff");
        assert_eq!(c["selection.background"], "#74ade83d");
        assert_eq!(c["danger.background"], "#d07277ff");
    }

    #[test]
    fn accent_gets_a_readable_foreground() {
        let dark = &converted()["themes"][0]["colors"];
        assert_eq!(dark["primary.foreground"], "#1b1f27", "light accent needs dark text");
        let light = &converted()["themes"][1]["colors"];
        assert_eq!(light["primary.foreground"], "#ffffff", "mid-dark accent needs white text");
    }

    #[test]
    fn syntax_and_editor_sections_are_mapped() {
        let h = &converted()["themes"][0]["highlight"];
        assert_eq!(h["editor.background"], "#282c33ff");
        assert_eq!(h["editor.line_number"], "#4e5a5fff");
        assert_eq!(h["error"], "#d07277ff");
        assert_eq!(h["syntax"]["keyword"]["color"], "#b477cfff");
        assert_eq!(h["syntax"]["comment"]["font_style"], "italic");
        assert_eq!(h["syntax"]["title"]["font_weight"], 400);
        assert!(h["syntax"]["broken"].is_null(), "entries without a colour are skipped");
    }

    #[test]
    fn sparse_theme_inherits_from_quarry_base() {
        let c = &converted()["themes"][1]["colors"];
        // Not stated by the fixture: must come from Quarry Light so the theme is complete.
        assert_eq!(c["border"], "#c9c9ca");
        assert_eq!(c["background"], "#fafafaff");
    }

    #[test]
    fn result_loads_with_the_real_theme_schema() {
        let set: gpui_kit::component::ThemeSet =
            serde_json::from_str(&import_zed_theme(FIXTURE).unwrap()).unwrap();
        assert_eq!(set.themes.len(), 2);
        assert!(set.themes[0].highlight.is_some());
        assert!(!set.themes[0].is_default);
    }

    #[test]
    fn invalid_input_is_an_error_not_a_panic() {
        assert!(import_zed_theme("nope").is_err());
        assert!(import_zed_theme(r#"{"name":"x"}"#).is_err());
        assert!(import_zed_theme(r#"{"themes":[]}"#).is_err());
        assert!(import_zed_theme(r#"{"themes":[{"name":"a"}]}"#).is_err());
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(slug("One Dark (Test)!"), "one-dark-test");
        assert_eq!(slug("???"), "theme");
    }

    #[test]
    fn hex_helpers() {
        assert_eq!(parse_hex("#fff"), Some((255, 255, 255, 255)));
        assert_eq!(parse_hex("#11223344"), Some((0x11, 0x22, 0x33, 0x44)));
        assert_eq!(parse_hex("red"), None);
        assert_eq!(to_hex((1, 2, 3, 255)), "#010203");
    }
}
