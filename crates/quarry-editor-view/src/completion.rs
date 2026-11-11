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
    /// The user moved the selection (arrows, wheel hover, click): from then on Enter accepts.
    pub navigated: bool,
}

impl CompletionState {
    /// `None` when there is nothing to show.
    pub fn new(list: CompletionList, replace: Range<usize>, prefix: String) -> Option<Self> {
        if list.items.is_empty() {
            return None;
        }
        Some(CompletionState {
            items: list.items,
            selected: 0,
            top: 0,
            replace,
            prefix,
            manual: false,
            navigated: false,
        })
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether Enter should leave the popup alone and insert a newline: the popup was opened by
    /// typing, nothing was picked, and the top suggestion is only a keyword. Typing an alias
    /// (`FROM customers c`) suggests `CROSS JOIN`; Enter must not turn the alias into that.
    pub fn enter_means_newline(&self) -> bool {
        !self.manual
            && !self.navigated
            && self.selected_item().is_some_and(|i| i.kind == CompletionKind::Keyword)
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
        self.navigated = true;
        self.ensure_visible();
    }

    pub fn select(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = index;
            self.navigated = true;
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

/// Whether the selected item would insert exactly what is already in the replace range.
/// Accepting it would change nothing, so Enter should fall through to a newline instead.
pub fn is_noop_accept(selected_insert: &str, current: &str) -> bool {
    !current.is_empty() && selected_insert.eq_ignore_ascii_case(current)
}

/// Whether a space just typed after `before` (the text up to and including that space) should open
/// completion by itself: right after `FROM`, any `JOIN`, `ON`, `INTO`, `UPDATE`, `TABLE`, and after
/// a comma inside a FROM list. Never after arbitrary spaces, which would make the popup noise.
pub fn auto_trigger_after_space(before: &str) -> bool {
    let Some(head) = before.strip_suffix(' ') else { return false };
    let head = head.trim_end_matches(' ');
    if let Some(prefix) = head.strip_suffix(',') {
        return in_from_list(prefix);
    }
    let word: String =
        head.chars().rev().take_while(|c| is_word_char(*c)).collect::<Vec<_>>().into_iter().rev().collect();
    // The word must stand alone (`a.from` or `x_join` are identifiers, not keywords).
    let preceding = head[..head.len() - word.len()].chars().next_back();
    if preceding.is_some_and(|c| is_word_char(c) || c == '.' || c == '"' || c == '`') {
        return false;
    }
    matches!(word.to_ascii_lowercase().as_str(), "from" | "join" | "on" | "into" | "update" | "table")
}

/// Whether the last clause keyword before the end of `text` (in the current statement, outside
/// parentheses opened after it) is `FROM`.
fn in_from_list(text: &str) -> bool {
    let stmt = text.rsplit(';').next().unwrap_or(text);
    let mut depth = 0i32;
    let mut word = String::new();
    let mut last_clause: Option<String> = None;
    let flush = |word: &mut String, depth: i32, last: &mut Option<String>| {
        if depth == 0 && !word.is_empty() {
            let w = word.to_ascii_lowercase();
            if matches!(
                w.as_str(),
                "select" | "from" | "where" | "group" | "order" | "having" | "set" | "limit" | "values"
            ) {
                *last = Some(w);
            }
        }
        word.clear();
    };
    for c in stmt.chars() {
        if is_word_char(c) {
            word.push(c);
            continue;
        }
        flush(&mut word, depth, &mut last_clause);
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ => {}
        }
    }
    flush(&mut word, depth, &mut last_clause);
    depth == 0 && last_clause.as_deref() == Some("from")
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
    fn enter_after_an_alias_is_a_newline_unless_the_user_picked_something() {
        let kw = |l: &str| CompletionItem {
            label: l.into(),
            kind: CompletionKind::Keyword,
            detail: None,
            insert_text: l.into(),
            score: 0,
        };
        let list = CompletionList { replace_range: 0..1, items: vec![kw("CROSS JOIN"), kw("CASE")] };
        let mut s = CompletionState::new(list, 0..1, "c".into()).unwrap();
        assert!(s.enter_means_newline());
        s.move_selection(1);
        assert!(!s.enter_means_newline(), "after an explicit choice Enter accepts");
        // A table or column is accepted by Enter right away.
        let mut t = state(2);
        assert!(!t.enter_means_newline());
        t.manual = true;
        assert!(!t.enter_means_newline());
    }

    #[test]
    fn noop_accept_is_case_insensitive() {
        assert!(is_noop_accept("id", "id"));
        assert!(is_noop_accept("SELECT", "select"));
        assert!(!is_noop_accept("customers", "cu"));
        assert!(!is_noop_accept("id", ""));
    }

    #[test]
    fn auto_trigger_after_clause_keywords() {
        for t in [
            "select * from ",
            "select * from a JOIN ",
            "select * from a left join ",
            "select * from a join b ON ",
            "insert into ",
            "UPDATE ",
            "drop table ",
            "select * from a, ",
            "select * from a join b on a.x = b.x, ",
        ] {
            assert!(auto_trigger_after_space(t), "{t}");
        }
        for t in [
            "select ",
            "select a, ",
            "select * from a where ",
            "select * from a ",
            "select * from (select 1, ",
            "select a.from ",
            "select 1; select 2, ",
            "from",
        ] {
            assert!(!auto_trigger_after_space(t), "{t}");
        }
    }

    #[test]
    fn word_chars() {
        assert!(is_word_char('a') && is_word_char('_') && is_word_char('9') && is_word_char('é'));
        assert!(!is_word_char('.') && !is_word_char(' ') && !is_word_char('('));
    }
}
