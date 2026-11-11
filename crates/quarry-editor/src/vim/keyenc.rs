//! Keys <-> text, so macros can live in ordinary (pasteable) registers.
//!
//! Printable keys are themselves; Escape, Enter, Tab and Backspace are the control characters
//! Vim uses (`^[`, `^M`, `^I`, `^?`); other `ctrl-x` chords are the control characters; keys with
//! no character form (arrows, Home/End, F-keys, alt chords) use the private-use area.

use crate::keys::{Key, KeyInput, Modifiers};

const PUA: u32 = 0xF700;

pub(crate) fn encode(keys: &[KeyInput]) -> String {
    let mut s = String::new();
    for k in keys {
        let m = &k.modifiers;
        let c = match &k.key {
            Key::Char(c) if m.ctrl && !m.alt && !m.platform && c.is_ascii_alphabetic() => {
                ((c.to_ascii_lowercase() as u8) & 0x1f) as char
            }
            Key::Char(c) if m.ctrl && !m.alt && !m.platform && *c == '[' => '\x1b',
            Key::Char(c) if m.alt && !m.ctrl && c.is_ascii() => {
                char::from_u32(0xF800 + *c as u32).unwrap_or(*c)
            }
            Key::Char(c) if !m.ctrl && !m.alt && !m.platform => *c,
            Key::Escape => '\x1b',
            Key::Enter => '\r',
            Key::Tab => '\t',
            Key::Backspace => '\x7f',
            Key::Left => pua(0),
            Key::Right => pua(1),
            Key::Up => pua(2),
            Key::Down => pua(3),
            Key::Home => pua(4),
            Key::End => pua(5),
            Key::PageUp => pua(6),
            Key::PageDown => pua(7),
            Key::Delete => pua(8),
            Key::F(n) => char::from_u32(0xF900 + *n as u32).unwrap_or('\u{F900}'),
            _ => continue,
        };
        s.push(c);
    }
    s
}

fn pua(n: u32) -> char {
    char::from_u32(PUA + n).unwrap_or('\u{F700}')
}

pub(crate) fn decode(s: &str) -> Vec<KeyInput> {
    let mut out = Vec::new();
    for c in s.chars() {
        let k = match c {
            '\x1b' => KeyInput::new(Key::Escape),
            '\r' | '\n' => KeyInput::new(Key::Enter),
            '\t' => KeyInput::new(Key::Tab),
            '\x7f' => KeyInput::new(Key::Backspace),
            '\u{1}'..='\u{1a}' => KeyInput {
                key: Key::Char((b'a' + (c as u8 - 1)) as char),
                modifiers: Modifiers { ctrl: true, ..Default::default() },
            },
            '\u{F700}'..='\u{F708}' => KeyInput::new(match c as u32 - PUA {
                0 => Key::Left,
                1 => Key::Right,
                2 => Key::Up,
                3 => Key::Down,
                4 => Key::Home,
                5 => Key::End,
                6 => Key::PageUp,
                7 => Key::PageDown,
                _ => Key::Delete,
            }),
            '\u{F800}'..='\u{F87F}' => KeyInput {
                key: Key::Char(char::from_u32(c as u32 - 0xF800).unwrap_or('?')),
                modifiers: Modifiers { alt: true, ..Default::default() },
            },
            '\u{F900}'..='\u{F9FF}' => KeyInput::new(Key::F((c as u32 - 0xF900) as u8)),
            c => KeyInput::ch(c),
        };
        out.push(k);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let keys: Vec<KeyInput> =
            ["i", "h", "esc", "ret", "C-a", "left", "A-x", "tab", "backspace", "F5", "ö"]
                .iter()
                .map(|s| KeyInput::parse(s).unwrap())
                .collect();
        let s = encode(&keys);
        let back = decode(&s);
        assert_eq!(back, keys);
    }
}
