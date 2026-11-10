//! Completion popup state: selection, scrolling and label highlighting. No GPUI.

use quarry_sql::{CompletionItem, CompletionKind, CompletionList};
use quarry_ui::fuzzy::fuzzy_match;
use std::ops::Range;

/// Rows visible at once; the list scrolls beyond this.
pub const MAX_VISIBLE_ROWS: usize = 10;
/// A detail longer than this also appears, in full, in the footer of the selected row.
pub const LONG_DETAIL: usize = 36;

pub struct CompletionState {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
    /// First visible row.
    pub top: usize,
    /// Chars of the partial word being replaced.
    pub replace: Range<usize>,
    /// The partial word, used to emphasise matched characters.
    pub prefix: String,
    /// Opened by an explicit request (ctrl-space) rather than by typing: it may offer
    /// suggestions for an empty word (`JOIN |`) and is not subject to the typing gate.
    pub manual: bool,
}

impl CompletionState {
    /// `None` when there is nothing to show.
    pub fn new(list: CompletionList, replace: Range<usize>, prefix: String) -> Option<Self> {
        if list.items.is_empty() {
            return None;
        }
        Some(CompletionState { items: list.items, selected: 0, top: 0, replace, prefix, manual: false })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    pub fn visible(&self) -> Range<usize> {
        self.top..(self.top + MAX_VISIBLE_ROWS).min(self.items.len())
    }

    /// Move the selection, wrapping at both ends, keeping it inside the visible window.
    pub fn move_selection(&mut self, delta: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        self.ensure_visible();
    }

    pub fn select(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = index;
            self.ensure_visible();
        }
    }

    /// Scroll the window by whole rows (mouse wheel), leaving the selection alone.
    pub fn scroll_rows(&mut self, delta: isize) {
        let max_top = self.items.len().saturating_sub(MAX_VISIBLE_ROWS) as isize;
        self.top = (self.top as isize + delta).clamp(0, max_top) as usize;
    }

    fn ensure_visible(&mut self) {
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + MAX_VISIBLE_ROWS {
            self.top = self.selected + 1 - MAX_VISIBLE_ROWS;
        }
    }

    /// The footer text for the selected item: the full `ON` clause of a join suggestion, or a
    /// detail too long for its row.
    pub fn footer(&self) -> Option<&str> {
        let item = self.selected_item()?;
        if item.kind == CompletionKind::Join {
            return Some(&item.insert_text);
        }
        item.detail.as_deref().filter(|d| d.chars().count() > LONG_DETAIL)
    }
}

/// Split `label` into `(text, matched)` segments by the fuzzy match of `prefix`.
pub fn highlight_segments(label: &str, prefix: &str) -> Vec<(String, bool)> {
    let positions = if prefix.is_empty() { None } else { fuzzy_match(prefix, label) };
    let Some(m) = positions else { return vec![(label.to_string(), false)] };
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut next = m.positions.iter().copied().peekable();
    for (byte, ch) in label.char_indices() {
        let matched = next.peek() == Some(&byte);
        if matched {
            next.next();
        }
        match out.last_mut() {
            Some((s, flag)) if *flag == matched => s.push(ch),
            _ => out.push((ch.to_string(), matched)),
        }
    }
    out
}

/// Whether `c` continues an identifier (what "typing a word" means for auto-completion).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use quarry_sql::CompletionKind;

    fn item(label: &str, detail: Option<&str>) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            kind: CompletionKind::Table,
            detail: detail.map(str::to_string),
            insert_text: label.into(),
            score: 0,
        }
    }

    fn state(n: usize) -> CompletionState {
        let list = CompletionList {
            replace_range: 0..0,
            items: (0..n).map(|i| item(&format!("t{i}"), None)).collect(),
        };
        CompletionState::new(list, 0..0, String::new()).unwrap()
    }

    #[test]
    fn empty_list_gives_no_state() {
        let list = CompletionList { replace_range: 0..0, items: vec![] };
        assert!(CompletionState::new(list, 0..0, String::new()).is_none());
    }

    #[test]
    fn selection_wraps_and_scrolls_window() {
        let mut s = state(25);
        assert_eq!(s.visible(), 0..10);
        s.move_selection(-1);
        assert_eq!(s.selected, 24);
        assert_eq!(s.visible(), 15..25);
        s.move_selection(1);
        assert_eq!((s.selected, s.top), (0, 0));
        for _ in 0..12 {
            s.move_selection(1);
        }
        assert_eq!(s.selected, 12);
        assert_eq!(s.visible(), 3..13);
        // Moving up inside the window does not scroll; moving past its top does.
        s.move_selection(-9);
        assert_eq!((s.selected, s.top), (3, 3));
        s.move_selection(-1);
        assert_eq!((s.selected, s.top), (2, 2));
    }

    #[test]
    fn short_lists_never_scroll() {
        let mut s = state(4);
        s.scroll_rows(5);
        assert_eq!(s.top, 0);
        assert_eq!(s.visible(), 0..4);
    }

    #[test]
    fn wheel_scroll_clamps() {
        let mut s = state(30);
        s.scroll_rows(100);
        assert_eq!(s.top, 20);
        s.scroll_rows(-100);
        assert_eq!(s.top, 0);
    }

    #[test]
    fn footer_only_for_long_details() {
        let list = CompletionList {
            replace_range: 0..0,
            items: vec![
                item("a", Some("integer")),
                item("o", Some("orders o ON o.customer_id = customers.id AND o.deleted_at IS NULL")),
            ],
        };
        let mut s = CompletionState::new(list, 0..0, String::new()).unwrap();
        assert_eq!(s.footer(), None);
        s.move_selection(1);
        assert!(s.footer().unwrap().starts_with("orders o ON"));
    }

    #[test]
    fn join_items_always_show_their_full_clause() {
        let join = CompletionItem {
            label: "customers c".into(),
            kind: CompletionKind::Join,
            detail: Some("FK orders_customer_id_fkey".into()),
            insert_text: "customers c ON o.customer_id = c.id".into(),
            score: 0,
        };
        let list = CompletionList { replace_range: 0..0, items: vec![join] };
        let s = CompletionState::new(list, 0..0, String::new()).unwrap();
        assert_eq!(s.footer(), Some("customers c ON o.customer_id = c.id"));
    }

    #[test]
    fn fuzzy_segments() {
        let segs = highlight_segments("customers", "cu");
        assert_eq!(segs[0], ("cu".to_string(), true));
        assert_eq!(segs[1], ("stomers".to_string(), false));
        let segs = highlight_segments("order_items", "oi");
        assert_eq!(segs.iter().filter(|(_, m)| *m).map(|(s, _)| s.as_str()).collect::<String>(), "oi");
        assert_eq!(highlight_segments("abc", ""), vec![("abc".to_string(), false)]);
        assert_eq!(highlight_segments("abc", "zzz"), vec![("abc".to_string(), false)]);
    }

    #[test]
    fn word_chars() {
        assert!(is_word_char('a') && is_word_char('_') && is_word_char('9') && is_word_char('é'));
        assert!(!is_word_char('.') && !is_word_char(' ') && !is_word_char('('));
    }
}
