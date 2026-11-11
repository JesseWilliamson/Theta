//! Key input types and a small parser used by tests and by the keymap tables.

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Cmd on macOS / Super elsewhere
    pub platform: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Key {
    /// The produced character, already shifted: 'A', '%', ' '
    Char(char),
    Enter,
    Escape,
    Backspace,
    Delete,
    Tab,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyInput {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyInput {
    pub fn new(key: Key) -> Self {
        KeyInput { key, modifiers: Modifiers::default() }
    }

    pub fn ch(c: char) -> Self {
        KeyInput::new(Key::Char(c))
    }

    /// Parse "ctrl-d", "alt-;", "A", "space", "ret", "esc", "cmd-shift-p". Helix-style
    /// prefixes (`C-d`, `A-;`, `S-tab`) are accepted too.
    pub fn parse(s: &str) -> Option<KeyInput> {
        let mut mods = Modifiers::default();
        let mut rest = s;
        while let Some(idx) = rest.find('-') {
            if idx == 0 || idx + 1 >= rest.len() {
                break; // a bare "-" key, or a trailing dash
            }
            match &rest[..idx] {
                "ctrl" | "C" => mods.ctrl = true,
                "alt" | "A" | "M" | "meta" | "opt" | "option" => mods.alt = true,
                "shift" | "S" => mods.shift = true,
                "cmd" | "super" | "platform" | "win" => mods.platform = true,
                _ => break,
            }
            rest = &rest[idx + 1..];
        }
        let key = match rest {
            "space" => Key::Char(' '),
            "ret" | "enter" | "return" => Key::Enter,
            "esc" | "escape" => Key::Escape,
            "tab" => Key::Tab,
            "backspace" | "bs" => Key::Backspace,
            "del" | "delete" => Key::Delete,
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "minus" => Key::Char('-'),
            "lt" => Key::Char('<'),
            "gt" => Key::Char('>'),
            r if r.len() >= 2 && r.starts_with('F') && r[1..].chars().all(|c| c.is_ascii_digit()) => {
                Key::F(r[1..].parse().ok()?)
            }
            r if r.chars().count() == 1 => Key::Char(r.chars().next()?),
            _ => return None,
        };
        Some(KeyInput { key, modifiers: mods }.canon())
    }

    /// Canonical form: for character keys the case is carried by the char itself and the
    /// `shift` flag is dropped (`shift-a` == `A`), so tables and hosts agree.
    pub fn canon(mut self) -> Self {
        if let Key::Char(c) = self.key {
            if self.modifiers.shift {
                self.key = Key::Char(c.to_uppercase().next().unwrap_or(c));
            }
            self.modifiers.shift = false;
        }
        self
    }

    /// True when either `ctrl` or `platform` (cmd) is held, the "primary" modifier of the Standard keymap.
    pub fn primary(&self) -> bool {
        self.modifiers.ctrl || self.modifiers.platform
    }

    /// Typed text produced by this key (no ctrl/alt/platform), if any.
    pub fn text_char(&self) -> Option<char> {
        match self.key {
            Key::Char(c) if !self.modifiers.ctrl && !self.modifiers.alt && !self.modifiers.platform => {
                Some(c)
            }
            _ => None,
        }
    }

    /// Human-readable form for `pending_keys`, e.g. "ctrl-k", "g", "space".
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.modifiers.ctrl {
            s.push_str("ctrl-");
        }
        if self.modifiers.alt {
            s.push_str("alt-");
        }
        if self.modifiers.platform {
            s.push_str("cmd-");
        }
        if self.modifiers.shift {
            s.push_str("shift-");
        }
        match &self.key {
            Key::Char(' ') => s.push_str("space"),
            Key::Char(c) => s.push(*c),
            Key::Enter => s.push_str("ret"),
            Key::Escape => s.push_str("esc"),
            Key::Backspace => s.push_str("backspace"),
            Key::Delete => s.push_str("del"),
            Key::Tab => s.push_str("tab"),
            Key::Left => s.push_str("left"),
            Key::Right => s.push_str("right"),
            Key::Up => s.push_str("up"),
            Key::Down => s.push_str("down"),
            Key::Home => s.push_str("home"),
            Key::End => s.push_str("end"),
            Key::PageUp => s.push_str("pageup"),
            Key::PageDown => s.push_str("pagedown"),
            Key::F(n) => s.push_str(&format!("F{n}")),
        }
        s
    }
}
