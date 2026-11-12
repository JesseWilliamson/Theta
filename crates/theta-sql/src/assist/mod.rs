//! Editor assistance built on the syntax tree and the resolver: quick fixes and refactorings, go to
//! definition, find usages and rename.
//!
//! Everything here is a pure function of `(text, analysis, cursor)` that returns *text edits*; the editor
//! applies them as one undo step. Offsets are byte offsets into the analysed text and edits never overlap, so a
//! caller can apply them back to front ([`apply_edits`]). Each action either is certain to produce valid SQL
//! for the statement it was offered in, or is not offered: there is no "best effort" refactoring.

mod fixes;
mod navigate;
mod rename;

pub use fixes::{Fix, FixKind, code_actions};
pub use navigate::{Definition, Target, Usage, definition_at, highlights_at, target_at, usages_of};
pub use rename::{rename_at, rename_prepare};

use std::ops::Range;

/// Replace `range` of the text with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Edit {
        Edit { range, text: text.into() }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Edit {
        Edit { range: at..at, text: text.into() }
    }

    pub fn delete(range: Range<usize>) -> Edit {
        Edit { range, text: String::new() }
    }
}

/// Apply non-overlapping edits to `sql` (they may come in any order).
pub fn apply_edits(sql: &str, edits: &[Edit]) -> String {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| (e.range.start, e.range.end));
    let mut out = String::with_capacity(sql.len());
    let mut at = 0;
    for e in sorted {
        debug_assert!(e.range.start >= at, "overlapping edits");
        let start = e.range.start.max(at);
        out.push_str(&sql[at..start]);
        out.push_str(&e.text);
        at = e.range.end.max(at);
    }
    out.push_str(&sql[at..]);
    out
}

#[cfg(test)]
mod tests;
