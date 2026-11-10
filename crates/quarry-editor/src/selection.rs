//! Selections.
//!
//! The model is Helix's: a selection is `anchor`..`head`, half-open, and the "cursor" of a
//! forward selection is the *last grapheme inside it* (a 1-wide block). A selection that
//! covers exactly one grapheme is what other editors call "a cursor at that character".
//! The Standard keymap uses the same type but with a bar cursor: `cursor()` is `head`.

use crate::text::{self, Text};
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Selection { anchor, head }
    }

    pub fn point(pos: usize) -> Self {
        Selection { anchor: pos, head: pos }
    }

    /// The half-open char range to highlight.
    pub fn range(&self) -> Range<usize> {
        self.from()..self.to()
    }

    /// Where the cursor is drawn, assuming the Helix block convention (`head - 1` for a
    /// forward selection). This is exact for ASCII and for single-char graphemes; hosts that
    /// need grapheme-exactness or the Standard bar cursor should use `Editor::cursor_of`.
    pub fn cursor(&self) -> usize {
        if self.head > self.anchor { self.head - 1 } else { self.head }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn len(&self) -> usize {
        self.to() - self.from()
    }

    pub fn is_forward(&self) -> bool {
        self.head >= self.anchor
    }

    pub fn flip(&self) -> Self {
        Selection { anchor: self.head, head: self.anchor }
    }

    pub fn with_direction(&self, forward: bool) -> Self {
        if self.is_empty() || self.is_forward() == forward {
            *self
        } else {
            self.flip()
        }
    }

    // ---- Helix block-cursor semantics ------------------------------------------------------

    /// Grapheme-exact cursor index in the Helix convention.
    pub fn block_cursor(&self, t: Text) -> usize {
        if self.head > self.anchor { text::prev_grapheme(t, self.head) } else { self.head }
    }

    /// Widen an empty selection to the grapheme it sits on (impossible at end of text).
    pub fn min_width_1(&self, t: Text) -> Self {
        if self.anchor == self.head && self.head < t.len_chars() {
            Selection { anchor: self.anchor, head: text::next_grapheme(t, self.head) }
        } else {
            *self
        }
    }

    pub fn grapheme_aligned(&self, t: Text) -> Self {
        let len = t.len_chars();
        let a = text::floor_grapheme(t, self.anchor.min(len));
        let h = text::floor_grapheme(t, self.head.min(len));
        if a == h {
            return Selection { anchor: a, head: h };
        }
        if a < h {
            // The end of a forward range is exclusive: round it *up* so the last grapheme stays inside.
            let h2 = if h == self.head.min(len) { h } else { text::next_grapheme(t, h) };
            Selection { anchor: a, head: h2 }
        } else {
            let a2 = if a == self.anchor.min(len) { a } else { text::next_grapheme(t, a) };
            Selection { anchor: a2, head: h }
        }
    }

    /// Move the block cursor to `pos` (extending keeps the anchor, with Helix's 1-width fix-ups).
    pub fn put_cursor(&self, t: Text, pos: usize, extend: bool) -> Self {
        if !extend {
            return Selection::point(pos);
        }
        let anchor = if self.head >= self.anchor && pos < self.anchor {
            text::next_grapheme(t, self.anchor)
        } else if self.head < self.anchor && pos >= self.anchor {
            text::prev_grapheme(t, self.anchor)
        } else {
            self.anchor
        };
        if anchor <= pos { Selection::new(anchor, text::next_grapheme(t, pos)) } else { Selection::new(anchor, pos) }
    }

    /// First and last line touched by the selection (the end is exclusive-aware).
    pub fn line_range(&self, t: Text) -> (usize, usize) {
        let from = self.from();
        let to = if self.is_empty() { self.to() } else { text::prev_grapheme(t, self.to()).max(from) };
        (t.char_to_line(from), t.char_to_line(to))
    }
}

/// An ordered, non-overlapping list of selections with a primary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selections {
    pub v: Vec<Selection>,
    pub primary: usize,
}

impl Selections {
    pub fn single(s: Selection) -> Self {
        Selections { v: vec![s], primary: 0 }
    }

    pub fn new(v: Vec<Selection>, primary: usize) -> Self {
        let mut s = Selections { v, primary };
        if s.v.is_empty() {
            s.v.push(Selection::point(0));
            s.primary = 0;
        }
        s.primary = s.primary.min(s.v.len() - 1);
        s.normalize();
        s
    }

    pub fn primary(&self) -> Selection {
        self.v[self.primary]
    }

    /// Sort by position and merge overlapping selections; the primary follows its selection.
    pub fn normalize(&mut self) {
        if self.v.len() <= 1 {
            return;
        }
        let mut idx: Vec<usize> = (0..self.v.len()).collect();
        idx.sort_by_key(|&i| (self.v[i].from(), self.v[i].to()));
        let mut out: Vec<Selection> = Vec::with_capacity(idx.len());
        let mut primary = 0;
        for &i in &idx {
            let cur = self.v[i];
            if let Some(last) = out.last_mut() {
                // Touching empties at the same spot and true overlaps merge; merely adjacent
                // non-empty selections stay separate (they select different text).
                let overlap = cur.from() < last.to() || (cur.from() == last.to() && (cur.is_empty() || last.is_empty()));
                if overlap {
                    let (from, to) = (last.from(), last.to().max(cur.to()));
                    *last = if cur.is_forward() { Selection::new(from, to) } else { Selection::new(to, from) };
                    if i == self.primary {
                        primary = out.len() - 1;
                    }
                    continue;
                }
            }
            if i == self.primary {
                primary = out.len();
            }
            out.push(cur);
        }
        self.v = out;
        self.primary = primary;
    }

    pub fn len(&self) -> usize {
        self.v.len()
    }

    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }
}
