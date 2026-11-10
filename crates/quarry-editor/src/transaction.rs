//! Transactions: a sorted list of non-overlapping replacements applied atomically.

use ropey::Rope;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Change {
    pub from: usize,
    pub to: usize,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Assoc {
    /// A position sitting exactly at an insertion stays in front of the inserted text.
    Before,
    /// ... or moves behind it.
    After,
}

#[derive(Clone, Default, Debug)]
pub struct Transaction {
    /// Sorted by `from`, non-overlapping, in coordinates of the document before the transaction.
    pub changes: Vec<Change>,
}

impl Transaction {
    /// Build from unsorted changes. Overlapping changes are dropped (first wins) so a buggy
    /// command can never corrupt the buffer.
    pub fn new(mut changes: Vec<Change>) -> Self {
        changes.sort_by_key(|c| (c.from, c.to));
        let mut out: Vec<Change> = Vec::with_capacity(changes.len());
        for c in changes {
            if out.last().is_some_and(|last| c.from < last.to) {
                continue;
            }
            out.push(c);
        }
        Transaction { changes: out }
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// Apply to `rope` and return the inverse transaction (in post-change coordinates).
    pub fn apply(&self, rope: &mut Rope) -> Transaction {
        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut shift: isize = 0;
        for c in &self.changes {
            let removed = if c.to > c.from { rope.slice(c.from..c.to).to_string() } else { String::new() };
            let new_from = (c.from as isize + shift) as usize;
            let new_len = c.text.chars().count();
            inverse.push(Change { from: new_from, to: new_from + new_len, text: removed });
            shift += new_len as isize - (c.to - c.from) as isize;
        }
        for c in self.changes.iter().rev() {
            if c.to > c.from {
                rope.remove(c.from..c.to);
            }
            if !c.text.is_empty() {
                rope.insert(c.from, &c.text);
            }
        }
        Transaction { changes: inverse }
    }

    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut shift: isize = 0;
        for c in &self.changes {
            let len = c.text.chars().count();
            if pos < c.from {
                break;
            }
            if c.from == c.to {
                // pure insertion
                if pos == c.from && assoc == Assoc::Before {
                    break;
                }
                shift += len as isize;
                continue;
            }
            if pos > c.to {
                shift += len as isize - (c.to - c.from) as isize;
                continue;
            }
            // pos inside [from, to]. The start of a replacement stays put so that a same-size
            // replacement (r, ~, indent changes ...) keeps a selection covering the new text.
            if pos == c.from {
                return (pos as isize + shift) as usize;
            }
            let mapped = match assoc {
                Assoc::Before if pos < c.to => c.from,
                _ => c.from + len,
            };
            return (mapped as isize + shift) as usize;
        }
        (pos as isize + shift) as usize
    }

    /// First and last+1 char of the document (pre-change) touched.
    pub fn span(&self) -> Option<(usize, usize)> {
        Some((self.changes.first()?.from, self.changes.last()?.to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(from: usize, to: usize, text: &str) -> Change {
        Change { from, to, text: text.to_string() }
    }

    #[test]
    fn apply_and_invert_roundtrip() {
        let mut rope = Rope::from_str("hello world");
        let tx = Transaction::new(vec![ch(0, 5, "HEY"), ch(6, 11, ""), ch(11, 11, "!")]);
        let inv = tx.apply(&mut rope);
        assert_eq!(rope.to_string(), "HEY !");
        inv.apply(&mut rope);
        assert_eq!(rope.to_string(), "hello world");
    }

    #[test]
    fn overlapping_changes_are_dropped_not_applied() {
        let tx = Transaction::new(vec![ch(2, 6, "x"), ch(4, 8, "y")]);
        assert_eq!(tx.changes.len(), 1);
        assert_eq!(tx.changes[0].text, "x");
    }

    #[test]
    fn two_inserts_at_one_position_keep_their_order() {
        let mut rope = Rope::from_str("ab");
        Transaction::new(vec![ch(1, 1, "X"), ch(1, 1, "Y")]).apply(&mut rope);
        assert_eq!(rope.to_string(), "aXYb");
    }

    #[test]
    fn map_pos_insert_association() {
        let tx = Transaction::new(vec![ch(3, 3, "xx")]);
        assert_eq!(tx.map_pos(3, Assoc::Before), 3);
        assert_eq!(tx.map_pos(3, Assoc::After), 5);
        assert_eq!(tx.map_pos(2, Assoc::After), 2);
        assert_eq!(tx.map_pos(4, Assoc::Before), 6);
    }

    #[test]
    fn map_pos_through_deletion_and_replacement() {
        let del = Transaction::new(vec![ch(2, 6, "")]);
        assert_eq!(del.map_pos(4, Assoc::Before), 2);
        assert_eq!(del.map_pos(4, Assoc::After), 2);
        assert_eq!(del.map_pos(8, Assoc::After), 4);
        let rep = Transaction::new(vec![ch(2, 6, "abc")]);
        assert_eq!(rep.map_pos(2, Assoc::After), 2, "replacement start stays put");
        assert_eq!(rep.map_pos(6, Assoc::Before), 5);
        assert_eq!(rep.map_pos(4, Assoc::Before), 2);
        assert_eq!(rep.map_pos(4, Assoc::After), 5);
    }

    #[test]
    fn unicode_chars_not_bytes() {
        let mut rope = Rope::from_str("héllo");
        Transaction::new(vec![ch(1, 2, "e")]).apply(&mut rope);
        assert_eq!(rope.to_string(), "hello");
    }
}
