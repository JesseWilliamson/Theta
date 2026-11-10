//! Small public value types: what a status bar shows and what the gutter marks.

use quarry_editor::{Keymap, Mode};

/// Outcome mark shown in the gutter lane on the first line of a statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatementStatus {
    Running,
    Ok,
    Failed,
}

/// Everything a status bar needs about the editor, as plain data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorStatus {
    pub keymap: Keymap,
    pub mode: Mode,
    /// Keys typed so far toward a multi-key command (`g`, `"a`, `2d`, `ctrl-k`).
    pub pending_keys: String,
    /// 1-based line of the primary cursor.
    pub line: usize,
    /// 1-based column of the primary cursor, in chars.
    pub column: usize,
    pub selection_count: usize,
    /// Chars covered by real selections (a bare cursor counts as 0).
    pub selected_chars: usize,
    /// Latest engine status message (`yanked 2 lines`, command errors) while it is still fresh.
    pub message: Option<String>,
    pub modified: bool,
}

impl EditorStatus {
    /// `NOR`, `INS`, `SEL` or `STD`: the three-letter mode tag status bars show.
    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "NOR",
            Mode::Insert => "INS",
            Mode::Select => "SEL",
            Mode::Standard => "STD",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_labels() {
        let mk = |mode| EditorStatus {
            keymap: Keymap::Helix,
            mode,
            pending_keys: String::new(),
            line: 1,
            column: 1,
            selection_count: 1,
            selected_chars: 0,
            message: None,
            modified: false,
        };
        assert_eq!(mk(Mode::Normal).mode_label(), "NOR");
        assert_eq!(mk(Mode::Insert).mode_label(), "INS");
        assert_eq!(mk(Mode::Select).mode_label(), "SEL");
        assert_eq!(mk(Mode::Standard).mode_label(), "STD");
    }
}
