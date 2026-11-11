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

/// The single contiguous edit that turns `old` into `new`: `(start_char, removed_chars,
/// inserted_chars)`, found by trimming the common prefix and suffix. With several cursors the
/// result is the hull between the outermost edits, which only makes callers more conservative.
pub fn diff_edit(old: &str, new: &str) -> Option<(usize, usize, usize)> {
    if old == new {
        return None;
    }
    let (ob, nb) = (old.as_bytes(), new.as_bytes());
    let mut prefix = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = ob.len().min(nb.len()) - prefix;
    let mut suffix = ob.iter().rev().zip(nb.iter().rev()).take(max_suffix).take_while(|(a, b)| a == b).count();
    while !old.is_char_boundary(ob.len() - suffix) || !new.is_char_boundary(nb.len() - suffix) {
        suffix -= 1;
    }
    let start = old[..prefix].chars().count();
    let removed = old[prefix..ob.len() - suffix].chars().count();
    let inserted = new[prefix..nb.len() - suffix].chars().count();
    Some((start, removed, inserted))
}

/// Carry a char range through an edit that replaced `removed` chars at `start` with `inserted`
/// chars. `None` when the edit touched the range (including typing right at its end), because
/// the diagnostic then describes text that no longer exists.
pub fn shift_range(
    r: std::ops::Range<usize>,
    start: usize,
    removed: usize,
    inserted: usize,
) -> Option<std::ops::Range<usize>> {
    let edit_end = start + removed;
    if r.end < start || (r.end == start && removed > 0) {
        return Some(r);
    }
    if r.start >= edit_end && !(r.start == start && removed == 0 && r.end == start) {
        let d = |x: usize| x + inserted - removed;
        return Some(d(r.start)..d(r.end));
    }
    None
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    #[test]
    fn diff_finds_the_edit() {
        assert_eq!(diff_edit("abc", "abc"), None);
        assert_eq!(diff_edit("select 1", "select 12"), Some((8, 0, 1)));
        assert_eq!(diff_edit("select 1", "elect 1"), Some((0, 1, 0)));
        assert_eq!(diff_edit("a héllo b", "a hèllo b"), Some((3, 1, 1)));
        assert_eq!(diff_edit("aaa", "aaaa"), Some((3, 0, 1)));
    }

    #[test]
    fn ranges_move_with_edits_and_drop_when_touched() {
        // Insert before: shifts.
        assert_eq!(shift_range(10..14, 2, 0, 3), Some(13..17));
        // Delete before: shifts back.
        assert_eq!(shift_range(10..14, 2, 4, 0), Some(6..10));
        // Insert after: unchanged.
        assert_eq!(shift_range(10..14, 20, 0, 5), Some(10..14));
        // Typing at the end of the word, inside it, or replacing part of it drops it.
        assert_eq!(shift_range(10..14, 14, 0, 1), None);
        assert_eq!(shift_range(10..14, 12, 0, 1), None);
        assert_eq!(shift_range(10..14, 13, 3, 1), None);
        // Insert exactly at the start pushes the range right without touching its text.
        assert_eq!(shift_range(10..14, 10, 0, 2), Some(12..16));
        // A deletion that ends where the range starts leaves it intact.
        assert_eq!(shift_range(10..14, 7, 3, 0), Some(7..11));
    }
}
