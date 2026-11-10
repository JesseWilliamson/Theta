//! Key-binding chips (`⌃↵` on macOS, `Ctrl+Enter` elsewhere).

use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, px,
};

/// Format a GPUI keystroke string (`"ctrl-shift-p"`, `"cmd-enter"`, a chord
/// like `"ctrl-k ctrl-c"`) for display.
pub fn format_keystrokes(spec: &str, mac: bool) -> String {
    spec.split_whitespace().map(|s| format_one(s, mac)).collect::<Vec<_>>().join(" ")
}

fn format_one(stroke: &str, mac: bool) -> String {
    // The key is whatever follows the last '-' that is not itself the key
    // ("ctrl--" binds the minus key).
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = stroke;
    let key;
    loop {
        match rest.find('-') {
            Some(i) if i > 0 && i + 1 < rest.len() => {
                parts.push(&rest[..i]);
                rest = &rest[i + 1..];
            }
            _ => {
                key = rest;
                break;
            }
        }
    }
    let (mut ctrl, mut alt, mut shift, mut cmd) = (false, false, false, false);
    for p in parts {
        match p {
            "ctrl" | "control" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            "cmd" | "super" | "win" | "meta" | "secondary" => cmd = true,
            _ => {}
        }
    }
    let key_label = key_label(key, mac);
    if mac {
        let mut s = String::new();
        if ctrl {
            s.push('⌃');
        }
        if alt {
            s.push('⌥');
        }
        if shift {
            s.push('⇧');
        }
        if cmd {
            s.push('⌘');
        }
        s + &key_label
    } else {
        let mut v: Vec<String> = Vec::new();
        if ctrl {
            v.push("Ctrl".into());
        }
        if alt {
            v.push("Alt".into());
        }
        if shift {
            v.push("Shift".into());
        }
        if cmd {
            v.push("Super".into());
        }
        v.push(key_label);
        v.join("+")
    }
}

fn key_label(key: &str, mac: bool) -> String {
    let named = match (key, mac) {
        ("enter", true) => "↵",
        ("enter", false) => "Enter",
        ("escape", true) => "⎋",
        ("escape", false) => "Esc",
        ("tab", true) => "⇥",
        ("tab", false) => "Tab",
        ("backspace", true) => "⌫",
        ("backspace", false) => "Backspace",
        ("delete", true) => "⌦",
        ("delete", false) => "Del",
        ("space", _) => "Space",
        ("up", _) => "↑",
        ("down", _) => "↓",
        ("left", _) => "←",
        ("right", _) => "→",
        ("pageup", _) => "PgUp",
        ("pagedown", _) => "PgDn",
        ("home", _) => "Home",
        ("end", _) => "End",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    let mut chars = key.chars();
    match chars.next() {
        // One character: show it upper-case (`p` -> `P`). F-keys and anything
        // else keep their remaining characters (`f1` -> `F1`).
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Whether to use macOS glyphs.
pub fn is_mac() -> bool {
    cfg!(target_os = "macos")
}

/// A muted rounded chip showing a key binding.
#[derive(IntoElement)]
pub struct KeyHint {
    text: SharedString,
}

impl KeyHint {
    /// From a GPUI keystroke string such as `"ctrl-enter"`.
    pub fn new(spec: &str) -> Self {
        Self { text: format_keystrokes(spec, is_mac()).into() }
    }
    /// From text that is already formatted.
    pub fn from_label(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for KeyHint {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .h(px(18.))
            .px(px(5.))
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .text_size(px(11.))
            .text_color(theme.muted_foreground)
            .child(div().child(self.text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_for_non_mac() {
        assert_eq!(format_keystrokes("ctrl-enter", false), "Ctrl+Enter");
        assert_eq!(format_keystrokes("ctrl-shift-p", false), "Ctrl+Shift+P");
        assert_eq!(format_keystrokes("f1", false), "F1");
        assert_eq!(format_keystrokes("ctrl-k ctrl-c", false), "Ctrl+K Ctrl+C");
        assert_eq!(format_keystrokes("alt-up", false), "Alt+↑");
    }

    #[test]
    fn formats_for_mac() {
        assert_eq!(format_keystrokes("ctrl-enter", true), "⌃↵");
        assert_eq!(format_keystrokes("cmd-shift-p", true), "⇧⌘P");
        assert_eq!(format_keystrokes("cmd-alt-left", true), "⌥⌘←");
    }

    #[test]
    fn minus_key_is_not_a_separator() {
        assert_eq!(format_keystrokes("ctrl--", false), "Ctrl+-");
        assert_eq!(format_keystrokes("cmd-=", true), "⌘=");
    }
}
