//! Grapheme helpers shared by the controller and the painter.

use crate::display::DisplayLine;
use quarry_editor::{Editor, Keymap, Selection};
use unicode_segmentation::UnicodeSegmentation;

/// Chars in the first grapheme cluster of `s` (0 for an empty string).
pub fn first_grapheme_chars(s: &str) -> usize {
    s.graphemes(true).next().map_or(0, |g| g.chars().count())
}

/// Does `s` cover more than a bare cursor? Standard: any non-empty range. Helix: a selection
/// is a cursor when it is exactly one grapheme wide (the block cursor *is* that selection).
pub fn selection_is_real(editor: &Editor, s: &Selection) -> bool {
    if s.is_empty() {
        return false;
    }
    match editor.keymap() {
        Keymap::Standard => true,
        Keymap::Helix => {
            let end = (s.from() + 64).min(editor.len_chars());
            s.len() > first_grapheme_chars(&editor.slice(s.from()..end)).max(1)
        }
    }
}

/// `(end_col, end_byte)` of the grapheme that starts at source char column `col`: how wide a
/// block cursor drawn there must be. At the end of the line the extent is empty (callers
/// substitute one cell).
pub fn grapheme_extent(d: &DisplayLine, col: usize) -> (usize, usize) {
    if col >= d.char_count() {
        return (col, d.text.len());
    }
    let b = d.byte_of_char(col);
    let bn = d.byte_of_char(col + 1);
    // An expanded tab: one source char, several display spaces.
    if bn - b > 1 && d.text.as_bytes()[b..bn].iter().all(|&x| x == b' ') {
        return (col + 1, bn);
    }
    let len = d.text[b..].graphemes(true).next().map_or(1, str::len);
    (d.char_of_byte(b + len), b + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grapheme_clusters() {
        assert_eq!(first_grapheme_chars(""), 0);
        assert_eq!(first_grapheme_chars("abc"), 1);
        // Family emoji: 5 chars joined by ZWJ is one cluster.
        assert_eq!(first_grapheme_chars("👨\u{200d}👩\u{200d}👧x"), 5);
        // e + combining acute.
        assert_eq!(first_grapheme_chars("e\u{301}x"), 2);
    }

    #[test]
    fn extent_of_wide_and_combining_clusters() {
        let d = DisplayLine::new("a日e\u{301}b", 4);
        assert_eq!(grapheme_extent(&d, 0), (1, 1));
        assert_eq!(grapheme_extent(&d, 1), (2, 4));
        // 'e' + combining mark is one cluster of two chars.
        assert_eq!(grapheme_extent(&d, 2), (4, 7));
        assert_eq!(grapheme_extent(&d, 4), (5, 8));
        // End of line: empty extent.
        assert_eq!(grapheme_extent(&d, 5).0, 5);
    }

    #[test]
    fn extent_of_a_tab_is_its_expansion() {
        let d = DisplayLine::new("\tx", 4);
        assert_eq!(grapheme_extent(&d, 0), (1, 4));
        assert_eq!(grapheme_extent(&d, 1), (2, 5));
    }

    #[test]
    fn real_selections() {
        let mut e = Editor::new("hello world", Keymap::Helix);
        // Fresh Helix cursor: one grapheme wide, bare.
        assert!(!selection_is_real(&e, &e.primary()));
        e.set_selections(vec![Selection::new(0, 5)], 0);
        assert!(selection_is_real(&e, &e.primary()));
        let mut s = Editor::new("hello", Keymap::Standard);
        assert!(!selection_is_real(&s, &s.primary()));
        s.set_selections(vec![Selection::new(0, 1)], 0);
        assert!(selection_is_real(&s, &s.primary()));
    }
}
