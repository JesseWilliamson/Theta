//! Staged editing: the inline cell editor and row add/delete/undo/submit.

use crate::format::raw_text;
use crate::grid::{CellEditor, ResultGrid};
use crate::parse::parse_input;
use crate::pending::{RowRef, RowState};
use crate::selection::{Cell, Motion, Selection};
use crate::types::GridEvent;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::{AppContext as _, Context, SharedString, Window};
use quarry_core::{TypeKind, Value};

/// Where the cursor goes after a successful commit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum After {
    Stay,
    Right,
    Left,
}

impl ResultGrid {
    pub(crate) fn is_editable(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn can_edit_cursor(&self) -> bool {
        let (Some(sel), Some(p)) = (self.sel, &self.pending) else { return false };
        let rr = self.row_ref(sel.cursor.row);
        p.can_edit_column(sel.cursor.col)
            && p.row_state(rr) != RowState::Deleted
            && self.column_kind(sel.cursor.col) != TypeKind::Array
    }

    /// Open the inline editor on the current cell. `initial` replaces the
    /// prefill (type-to-edit); otherwise the value's text form is selected.
    pub(crate) fn start_edit(
        &mut self,
        initial: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit_cursor() {
            return;
        }
        let Some(sel) = self.sel else { return };
        let cell = sel.cursor;
        self.reveal(cell);
        let row = self.row_ref(cell.row);
        let typed = initial.is_some();
        let text = initial.unwrap_or_else(|| raw_text(self.value_at(cell.row, cell.col)));
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let subs = vec![cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                this.commit_edit(After::Stay, window, cx);
            }
            InputEvent::Blur => {
                // Clicking another cell commits first; a blur that arrives
                // later (window switch) must not discard what was typed, but
                // an invalid value cannot be committed, so it is dropped.
                if this.editor.is_some() && !this.commit_edit(After::Stay, window, cx) {
                    this.cancel_edit(window, cx);
                }
            }
            InputEvent::Change => {
                if let Some(e) = this.editor.as_mut()
                    && e.error.take().is_some()
                {
                    cx.notify();
                }
            }
            InputEvent::Focus => {}
        })];
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            if !typed {
                s.select_all(window, cx);
            }
        });
        self.editor = Some(CellEditor { row, col: cell.col, input, error: None, _subs: subs });
        cx.notify();
    }

    /// Parse and stage the editor's text. Returns false (editor stays open
    /// with an inline error) when the text is not valid for the column type.
    pub(crate) fn commit_edit(&mut self, after: After, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.editor.as_ref() else { return true };
        let text = editor.input.read(cx).value();
        let (row, col) = (editor.row, editor.col);
        let kind = self.column_kind(col);
        let value = match parse_input(&text, kind) {
            Ok(v) => v,
            Err(message) => {
                if let Some(e) = self.editor.as_mut() {
                    e.error = Some(SharedString::from(message));
                }
                cx.notify();
                return false;
            }
        };
        self.editor = None;
        if let (Some(p), Some(rs)) = (self.pending.as_mut(), self.result.as_ref()) {
            p.set_cell(rs, row, col, value);
        }
        self.after_pending_change(cx);
        window.focus(&self.focus, cx);
        match after {
            After::Stay => {}
            After::Right => self.move_selection(Motion::Next, false, cx),
            After::Left => self.move_selection(Motion::Prev, false, cx),
        }
        true
    }

    pub(crate) fn cancel_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Stage NULL for every editable cell in the selection.
    pub(crate) fn set_null(&mut self, cx: &mut Context<Self>) {
        let (Some(range), Some(p), Some(rs)) =
            (self.current_range(), self.pending.as_mut(), self.result.as_ref())
        else {
            return;
        };
        let nv = self.view.as_ref().map_or(rs.rows.len(), Vec::len);
        for r in range.r0..=range.r1 {
            let rr = if r < nv {
                RowRef::Orig(self.view.as_ref().map_or(r, |v| v[r] as usize))
            } else {
                RowRef::New(r - nv)
            };
            for c in range.c0..=range.c1 {
                p.set_cell(rs, rr, c, Value::Null);
            }
        }
        self.after_pending_change(cx);
    }

    pub(crate) fn add_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.pending.as_mut() else { return };
        p.add_row();
        let col = p.target().editable_columns.first().copied().unwrap_or(0);
        self.after_pending_change(cx);
        let last = self.n_rows() - 1;
        self.set_selection(Some(Selection::single(Cell::new(last, col))), true, cx);
        window.focus(&self.focus, cx);
    }

    /// Mark the selected rows deleted (new rows are simply dropped).
    pub(crate) fn delete_selected_rows(&mut self, cx: &mut Context<Self>) {
        let Some(range) = self.current_range() else { return };
        let refs: Vec<RowRef> = (range.r0..=range.r1).map(|r| self.row_ref(r)).collect();
        let Some(p) = self.pending.as_mut() else { return };
        // Delete staged new rows from the end so earlier indexes stay valid.
        for rr in refs.iter().rev() {
            p.delete_row(*rr);
        }
        let nr = self.n_rows();
        if nr == 0 {
            self.sel = None;
        } else if let Some(s) = self.sel {
            self.sel = Some(s.clamped(nr, self.n_cols()));
        }
        self.after_pending_change(cx);
    }

    pub(crate) fn undo_edit(&mut self, cx: &mut Context<Self>) {
        if self.pending.as_mut().is_some_and(|p| p.undo()) {
            let (nr, nc) = (self.n_rows(), self.n_cols());
            if let Some(s) = self.sel {
                self.sel = (nr > 0).then(|| s.clamped(nr, nc));
            }
            self.after_pending_change(cx);
        }
    }

    pub(crate) fn revert_changes(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.pending.as_mut() {
            p.revert();
        }
        self.editor = None;
        self.submit_error = None;
        let (nr, nc) = (self.n_rows(), self.n_cols());
        if let Some(s) = self.sel {
            self.sel = (nr > 0).then(|| s.clamped(nr, nc));
        }
        self.after_pending_change(cx);
    }

    pub(crate) fn submit_changes(&mut self, cx: &mut Context<Self>) {
        let (Some(p), Some(rs)) = (&self.pending, &self.result) else { return };
        if p.is_empty() {
            return;
        }
        let changes = p.changes(rs);
        self.submit_error = None;
        cx.emit(GridEvent::SubmitChanges(changes));
        cx.notify();
    }

    fn after_pending_change(&mut self, cx: &mut Context<Self>) {
        self.cache_clear();
        self.inspector_dirty = true;
        self.refresh_summary_pub();
        cx.notify();
    }
}
