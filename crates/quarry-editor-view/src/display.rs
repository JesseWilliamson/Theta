//! Source line -> display line.
//!
//! The engine addresses text in chars; the shaper addresses UTF-8 bytes of the string it was
//! given. Tabs also have to be expanded because shaped glyph advances know nothing about tab
//! stops. `DisplayLine` owns that mapping so painting, hit-testing and IME bounds agree.

use std::sync::Arc;

pub const TAB_WIDTH: usize = 4;

#[derive(Clone, Debug)]
pub struct DisplayLine {
    /// Text handed to the shaper (tabs expanded).
    pub text: Arc<str>,
    /// Byte offset in `text` of every source char, plus a final entry for the end. `None`
    /// when source and display are identical ASCII (the overwhelmingly common case), where
    /// char index == byte index.
    map: Option<Arc<[u32]>>,
    chars: usize,
}

impl DisplayLine {
    pub fn new(src: &str, tab_width: usize) -> Self {
        let plain = src.bytes().all(|b| b.is_ascii() && b != b'\t');
        if plain {
            return DisplayLine { text: Arc::from(src), map: None, chars: src.len() };
        }
        let mut text = String::with_capacity(src.len() + 8);
        let mut map: Vec<u32> = Vec::with_capacity(src.len() + 1);
        let mut col = 0usize;
        for c in src.chars() {
            map.push(text.len() as u32);
            if c == '\t' {
                let n = tab_width - col % tab_width;
                for _ in 0..n {
                    text.push(' ');
                }
                col += n;
            } else {
                text.push(c);
                col += 1;
            }
        }
        map.push(text.len() as u32);
        let chars = map.len() - 1;
        DisplayLine { text: Arc::from(text), map: Some(map.into()), chars }
    }

    /// Number of source chars.
    pub fn char_count(&self) -> usize {
        self.chars
    }

    /// Display byte offset of source char `c` (clamped to the end).
    pub fn byte_of_char(&self, c: usize) -> usize {
        match &self.map {
            None => c.min(self.chars),
            Some(m) => m[c.min(self.chars)] as usize,
        }
    }

    /// Source char whose display bytes contain `byte` (the char starting at or before it).
    pub fn char_of_byte(&self, byte: usize) -> usize {
        match &self.map {
            None => byte.min(self.chars),
            Some(m) => match m.binary_search(&(byte as u32)) {
                Ok(i) => {
                    // Several chars can share a start offset only for expanded tabs
                    // (they all start at distinct offsets), so `i` is exact; clamp the end.
                    i.min(self.chars)
                }
                Err(i) => i.saturating_sub(1).min(self.chars),
            },
        }
    }
}

/// Convert a byte range of the whole document to a char range, clamping to the text and
/// snapping to char boundaries (the lint/server positions may land mid-character after an
/// edit raced the background job).
pub fn byte_range_to_chars(
    text_len_bytes: usize,
    byte_to_char: &dyn Fn(usize) -> usize,
    r: std::ops::Range<usize>,
) -> std::ops::Range<usize> {
    let s = r.start.min(text_len_bytes);
    let e = r.end.min(text_len_bytes).max(s);
    byte_to_char(s)..byte_to_char(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_identity() {
        let d = DisplayLine::new("select 1", 4);
        assert_eq!(&*d.text, "select 1");
        assert_eq!(d.char_count(), 8);
        assert_eq!(d.byte_of_char(3), 3);
        assert_eq!(d.char_of_byte(3), 3);
        assert_eq!(d.byte_of_char(99), 8);
    }

    #[test]
    fn tabs_expand_to_stops() {
        let d = DisplayLine::new("\tab\tc", 4);
        assert_eq!(&*d.text, "    ab  c");
        assert_eq!(d.char_count(), 5);
        assert_eq!(d.byte_of_char(0), 0);
        assert_eq!(d.byte_of_char(1), 4);
        assert_eq!(d.byte_of_char(3), 6); // second tab starts at col 6, advances to 8
        assert_eq!(d.byte_of_char(4), 8);
        assert_eq!(d.byte_of_char(5), 9);
        // A click in the middle of an expanded tab resolves to the tab char.
        assert_eq!(d.char_of_byte(2), 0);
        assert_eq!(d.char_of_byte(7), 3);
        assert_eq!(d.char_of_byte(8), 4);
    }

    #[test]
    fn multibyte_chars_map_to_byte_offsets() {
        // 'é' is 2 bytes, '日' 3, '🙂' 4.
        let d = DisplayLine::new("aé日🙂z", 4);
        assert_eq!(d.char_count(), 5);
        assert_eq!(d.byte_of_char(1), 1);
        assert_eq!(d.byte_of_char(2), 3);
        assert_eq!(d.byte_of_char(3), 6);
        assert_eq!(d.byte_of_char(4), 10);
        assert_eq!(d.byte_of_char(5), 11);
        assert_eq!(d.char_of_byte(6), 3);
        assert_eq!(d.char_of_byte(7), 3);
        assert_eq!(d.char_of_byte(10), 4);
    }

    #[test]
    fn empty_line() {
        let d = DisplayLine::new("", 4);
        assert_eq!(d.char_count(), 0);
        assert_eq!(d.byte_of_char(0), 0);
        assert_eq!(d.char_of_byte(0), 0);
    }

    #[test]
    fn diagnostic_byte_ranges_map_to_chars() {
        let text = "select 'é', 日本 from t";
        let b2c = |b: usize| text[..b.min(text.len())].chars().count();
        // "日本" starts after "select 'é', " (13 bytes: é is 2).
        let start = text.find('日').unwrap();
        let end = start + "日本".len();
        assert_eq!(byte_range_to_chars(text.len(), &b2c, start..end), 12..14);
        // Out-of-range bytes clamp instead of panicking.
        assert_eq!(byte_range_to_chars(text.len(), &b2c, 5..9999), 5..text.chars().count());
        assert_eq!(
            byte_range_to_chars(text.len(), &b2c, 9999..10000),
            text.chars().count()..text.chars().count()
        );
    }
}
