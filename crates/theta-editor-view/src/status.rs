//! Small public value types: what a status bar shows and what the gutter marks.

use theta_editor::{Keymap, Mode};

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
    /// Register of the macro being recorded (Vim `q{a-z}`), until `q` stops it.
    pub recording: Option<char>,
}

impl EditorStatus {
    /// The mode tag status bars show: `NOR` `INS` `SEL` (Helix), `STD`, or `NORMAL` `INSERT`
    /// `VISUAL` `V-LINE` `V-BLOCK` `REPLACE` (Vim). Mirrors `Editor::mode_label` for this status.
    pub fn mode_label(&self) -> &'static str {
        match (self.keymap, self.mode) {
            (_, Mode::Standard) => "STD",
            (Keymap::Vim, Mode::Normal) => "NORMAL",
            (Keymap::Vim, Mode::Insert) => "INSERT",
            (_, Mode::Visual) => "VISUAL",
            (_, Mode::VisualLine) => "V-LINE",
            (_, Mode::VisualBlock) => "V-BLOCK",
            (_, Mode::Replace) => "REPLACE",
            (_, Mode::Normal) => "NOR",
            (_, Mode::Insert) => "INS",
            (_, Mode::Select) => "SEL",
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
            recording: None,
        };
        assert_eq!(mk(Mode::Normal).mode_label(), "NOR");
        assert_eq!(mk(Mode::Insert).mode_label(), "INS");
        assert_eq!(mk(Mode::Select).mode_label(), "SEL");
        assert_eq!(mk(Mode::Standard).mode_label(), "STD");
        let vim = |mode| EditorStatus { keymap: Keymap::Vim, ..mk(mode) };
        assert_eq!(vim(Mode::Normal).mode_label(), "NORMAL");
        assert_eq!(vim(Mode::Insert).mode_label(), "INSERT");
        assert_eq!(vim(Mode::Visual).mode_label(), "VISUAL");
        assert_eq!(vim(Mode::VisualLine).mode_label(), "V-LINE");
        assert_eq!(vim(Mode::VisualBlock).mode_label(), "V-BLOCK");
        assert_eq!(vim(Mode::Replace).mode_label(), "REPLACE");
    }
}
