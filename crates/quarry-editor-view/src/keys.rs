//! GPUI keystroke -> `quarry_editor::KeyInput`.
//!
//! Kept free of GPUI types so the whole table is unit-testable: the caller flattens a
//! `Keystroke` into `(key, key_char, modifiers)`.
//!
//! The rules, and why:
//! * Named keys (`enter`, `left`, `f5` ...) map by name; shift stays a flag on them.
//! * Plain printable keys (no ctrl/alt/cmd) use `key_char`, the text the layout produced.
//!   That is what makes shifted symbols and non-US layouts work: the engine never has to
//!   know that shift-7 is `&` on a US board and `/` on a German one.
//! * With ctrl/alt/cmd held, `key_char` is unreliable (macOS option-s yields `ß`, Linux
//!   yields control codes), so the key name decides, and shift is applied with the US
//!   shifted-symbol table so `ctrl-shift-/` still arrives as `ctrl-?`.
//! * ctrl+alt together with a printable `key_char` is AltGr on Windows layouts: it is text.

use quarry_editor::{Key, KeyInput, Modifiers};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

impl KeyMods {
    fn any_command(&self) -> bool {
        self.control || self.alt || self.platform
    }
}

/// The character a US keyboard produces for shift + `base`.
pub fn us_shifted(base: char) -> char {
    match base {
        '`' => '~',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        c => c.to_uppercase().next().unwrap_or(c),
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// Translate one key-down. `None` means "not something the engine understands" (a bare
/// modifier press, a dead key, an IME composition string): the caller leaves it to the
/// platform text-input path.
pub fn translate_key(key: &str, key_char: Option<&str>, mods: KeyMods) -> Option<KeyInput> {
    let named = match key {
        "enter" => Some(Key::Enter),
        "escape" => Some(Key::Escape),
        "backspace" => Some(Key::Backspace),
        "delete" => Some(Key::Delete),
        "tab" => Some(Key::Tab),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "space" => Some(Key::Char(' ')),
        _ => None,
    };
    let modifiers =
        Modifiers { ctrl: mods.control, alt: mods.alt, shift: mods.shift, platform: mods.platform };
    if let Some(k) = named {
        // Plain space typed with shift is still a space; `canon` drops the flag for chars.
        return Some(KeyInput { key: k, modifiers }.canon());
    }
    if let Some(n) = key.strip_prefix('f').and_then(|d| d.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Some(KeyInput { key: Key::F(n), modifiers }.canon());
    }
    if matches!(key, "shift" | "control" | "alt" | "platform" | "function" | "capslock" | "insert") {
        return None;
    }

    // AltGr-style text: ctrl+alt with a produced printable character.
    if mods.control
        && mods.alt
        && !mods.platform
        && let Some(c) = key_char.and_then(single_char)
        && !c.is_control()
    {
        return Some(KeyInput::ch(c));
    }

    if !mods.any_command() {
        let c = match key_char {
            Some(s) => single_char(s)?,
            None => {
                let base = single_char(key)?;
                if mods.shift { us_shifted(base) } else { base }
            }
        };
        if c.is_control() && c != '\t' {
            return None;
        }
        return Some(KeyInput::ch(c));
    }

    let base = single_char(key)?;
    let c = if mods.shift { us_shifted(base) } else { base };
    Some(KeyInput { key: Key::Char(c), modifiers: Modifiers { shift: false, ..modifiers } }.canon())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: KeyMods = KeyMods { control: false, alt: false, shift: false, platform: false };
    const CTRL: KeyMods = KeyMods { control: true, ..NONE };
    const ALT: KeyMods = KeyMods { alt: true, ..NONE };
    const SHIFT: KeyMods = KeyMods { shift: true, ..NONE };
    const CMD: KeyMods = KeyMods { platform: true, ..NONE };

    fn t(key: &str, ch: Option<&str>, m: KeyMods) -> Option<String> {
        translate_key(key, ch, m).map(|k| k.display())
    }

    #[test]
    fn printable_uses_key_char() {
        assert_eq!(t("a", Some("a"), NONE).as_deref(), Some("a"));
        assert_eq!(t("a", Some("A"), SHIFT).as_deref(), Some("A"));
        assert_eq!(t("7", Some("&"), SHIFT).as_deref(), Some("&"));
        // A German board: shift-7 produces `/`, which the table must not second-guess.
        assert_eq!(t("7", Some("/"), SHIFT).as_deref(), Some("/"));
        // Non-US layout: the ASCII-equivalent key, the typed character wins.
        assert_eq!(t("q", Some("ๆ"), NONE).as_deref(), Some("ๆ"));
    }

    #[test]
    fn printable_without_key_char_falls_back_to_key() {
        assert_eq!(t("x", None, NONE).as_deref(), Some("x"));
        assert_eq!(t("1", None, SHIFT).as_deref(), Some("!"));
    }

    #[test]
    fn shifted_symbols_table() {
        for (base, shifted) in
            [('/', '?'), (';', ':'), ('\'', '"'), ('-', '_'), ('=', '+'), ('[', '{'), (',', '<'), ('.', '>')]
        {
            assert_eq!(us_shifted(base), shifted);
        }
        assert_eq!(us_shifted('a'), 'A');
    }

    #[test]
    fn named_keys() {
        assert_eq!(t("enter", Some("\n"), NONE).as_deref(), Some("ret"));
        assert_eq!(t("escape", None, NONE).as_deref(), Some("esc"));
        assert_eq!(t("backspace", None, NONE).as_deref(), Some("backspace"));
        assert_eq!(t("tab", Some("\t"), NONE).as_deref(), Some("tab"));
        assert_eq!(t("tab", None, SHIFT).as_deref(), Some("shift-tab"));
        assert_eq!(
            t("left", None, KeyMods { alt: true, shift: true, ..NONE }).as_deref(),
            Some("alt-shift-left")
        );
        assert_eq!(t("space", Some(" "), NONE).as_deref(), Some("space"));
        assert_eq!(t("pagedown", None, NONE).as_deref(), Some("pagedown"));
        assert_eq!(t("f5", None, NONE).as_deref(), Some("F5"));
        assert_eq!(t("f12", None, SHIFT).as_deref(), Some("shift-F12"));
    }

    #[test]
    fn command_chords_use_the_key_name() {
        assert_eq!(t("a", Some("\u{1}"), CTRL).as_deref(), Some("ctrl-a"));
        assert_eq!(t("s", Some("ß"), ALT).as_deref(), Some("alt-s"));
        assert_eq!(t("enter", None, CTRL).as_deref(), Some("ctrl-ret"));
        assert_eq!(
            t("enter", None, KeyMods { control: true, shift: true, ..NONE }).as_deref(),
            Some("ctrl-shift-ret")
        );
        assert_eq!(t("p", None, KeyMods { platform: true, shift: true, ..NONE }).as_deref(), Some("cmd-P"));
        assert_eq!(t("l", None, KeyMods { control: true, shift: true, ..NONE }).as_deref(), Some("ctrl-L"));
        assert_eq!(t("/", None, KeyMods { control: true, shift: true, ..NONE }).as_deref(), Some("ctrl-?"));
        assert_eq!(t(".", None, ALT).as_deref(), Some("alt-."));
        assert_eq!(t("space", None, CTRL).as_deref(), Some("ctrl-space"));
        assert_eq!(t("c", None, CMD).as_deref(), Some("cmd-c"));
    }

    #[test]
    fn altgr_is_text() {
        let altgr = KeyMods { control: true, alt: true, ..NONE };
        let k = translate_key("q", Some("@"), altgr).unwrap();
        assert_eq!(k, KeyInput::ch('@'));
    }

    #[test]
    fn modifier_only_and_dead_keys_are_ignored() {
        assert!(translate_key("shift", None, SHIFT).is_none());
        assert!(translate_key("control", None, CTRL).is_none());
        // Multi-char key_char: a composed sequence, not a key press.
        assert!(translate_key("a", Some("ab"), NONE).is_none());
        assert!(translate_key("unknown-key", None, NONE).is_none());
    }

    #[test]
    fn engine_canonical_form() {
        // `shift-a` and `A` must be the same input to the engine.
        assert_eq!(translate_key("a", Some("A"), SHIFT), Some(KeyInput::ch('A')));
        assert_eq!(translate_key("a", None, SHIFT), Some(KeyInput::ch('A')));
    }
}
