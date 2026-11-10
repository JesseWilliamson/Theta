//! Keyboard and pointer handling.

use crate::editing::After;
use crate::grid::{Drag, ROW_H, Resize, ResultGrid};
use crate::selection::{Cell, Extent, Motion, Selection};
use crate::types::GridEvent;
use gpui_kit::{
    Context, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollEvent, Window, px,
};
use quarry_ui::{KeymapMode, Settings};

impl ResultGrid {
    pub(crate) fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Only act when the grid itself has focus: the filter box, the cell
        // editor and the inspector text are descendants whose key events also
        // bubble here.
        if self.editor.is_some() || !self.focus.is_focused(window) {
            return;
        }
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let key = ks.key.as_str();
        let secondary = m.control || m.platform;
        let helix = Settings::get(cx).keymap == KeymapMode::Helix;
        let extend = m.shift || self.helix_visual;
        let page = self.page_rows();

        let handled = if secondary && !m.alt {
            match key {
                "a" => {
                    self.select_all(cx);
                    true
                }
                "c" => {
                    self.copy_selection(m.shift, cx);
                    true
                }
                "f" => {
                    self.open_filter(window, cx);
                    true
                }
                "z" => {
                    self.undo_edit(cx);
                    true
                }
                "0" => {
                    self.set_null(cx);
                    true
                }
                "enter" => {
                    self.submit_changes(cx);
                    true
                }
                "home" => {
                    self.move_selection(Motion::Top, m.shift, cx);
                    true
                }
                "end" => {
                    self.move_selection(Motion::Bottom, m.shift, cx);
                    true
                }
                "d" if helix => {
                    self.move_selection(Motion::PageDown(page / 2), extend, cx);
                    true
                }
                "u" if helix => {
                    self.move_selection(Motion::PageUp(page / 2), extend, cx);
                    true
                }
                _ => false,
            }
        } else if !m.alt {
            self.plain_key(key, m.shift, extend, helix, page, ks.key_char.as_deref(), window, cx)
        } else {
            false
        };
        if handled {
            cx.stop_propagation();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plain_key(
        &mut self,
        key: &str,
        shift: bool,
        extend: bool,
        helix: bool,
        page: usize,
        key_char: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let was_g = std::mem::take(&mut self.helix_g);
        let motion = match key {
            "up" => Some(Motion::Up),
            "down" => Some(Motion::Down),
            "left" => Some(Motion::Left),
            "right" => Some(Motion::Right),
            "home" => Some(Motion::RowStart),
            "end" => Some(Motion::RowEnd),
            "pageup" => Some(Motion::PageUp(page)),
            "pagedown" => Some(Motion::PageDown(page)),
            "tab" => Some(if shift { Motion::Prev } else { Motion::Next }),
            "k" if helix => Some(Motion::Up),
            "j" if helix => Some(Motion::Down),
            "h" if helix => Some(Motion::Left),
            "l" if helix => Some(Motion::Right),
            "g" if helix && shift => Some(Motion::PageDown(usize::MAX / 2)),
            "g" if helix && was_g => Some(Motion::PageUp(usize::MAX / 2)),
            _ => None,
        };
        // Tab never extends: shift-tab means "previous cell".
        let extend = extend && !matches!(key, "tab");
        if let Some(motion) = motion {
            self.move_selection(motion, extend, cx);
            return true;
        }
        match key {
            "g" if helix => {
                self.helix_g = true;
                true
            }
            "v" if helix => {
                self.helix_visual = !self.helix_visual;
                cx.notify();
                true
            }
            "y" if helix => {
                self.copy_selection(false, cx);
                self.helix_visual = false;
                true
            }
            "x" if helix => {
                if let Some(s) = self.sel {
                    let nc = self.n_cols();
                    self.set_selection(
                        Some(Selection::row(s.cursor.row, s.cursor.col.min(nc.saturating_sub(1)))),
                        false,
                        cx,
                    );
                }
                true
            }
            "u" if helix => {
                self.undo_edit(cx);
                true
            }
            "i" if helix && self.can_edit_cursor() => {
                self.start_edit(None, window, cx);
                true
            }
            "5" if helix && shift => {
                self.select_all(cx);
                true
            }
            "/" => {
                self.open_filter(window, cx);
                true
            }
            "escape" => {
                self.helix_visual = false;
                if let Some(s) = self.sel
                    && !self.current_range().is_some_and(|r| r.is_single())
                {
                    self.set_selection(Some(Selection::single(s.cursor)), false, cx);
                }
                cx.notify();
                true
            }
            "space" => {
                self.toggle_inspector(cx);
                true
            }
            "enter" | "f2" => {
                self.activate_cursor(key == "enter", window, cx);
                true
            }
            "delete" => {
                if self.is_editable() {
                    self.delete_selected_rows(cx);
                }
                true
            }
            _ => {
                // Type-to-edit, only where letters are not commands.
                if !helix
                    && let Some(ch) = key_char
                    && ch.chars().count() == 1
                    && !ch.chars().all(char::is_control)
                    && self.can_edit_cursor()
                {
                    self.start_edit(Some(ch.to_string()), window, cx);
                    return true;
                }
                false
            }
        }
    }

    /// Enter / F2 / double-click on the current cell.
    pub(crate) fn activate_cursor(&mut self, from_enter: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.can_edit_cursor() {
            self.start_edit(None, window, cx);
            return;
        }
        if let Some(sel) = self.sel {
            let col = sel.cursor.col;
            let value = self.value_at(sel.cursor.row, col).clone();
            if self.is_reference_column(col) && !value.is_null() {
                cx.emit(GridEvent::FollowReference { column: col, value });
                return;
            }
        }
        if from_enter || !self.inspector_open {
            if !self.inspector_open {
                self.toggle_inspector(cx);
            }
            self.inspector_dirty = true;
            cx.notify();
        }
    }

    // ------------------------------------------------------------------ pointer

    pub(crate) fn on_body_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        if self.editor.is_some() && !self.commit_edit(After::Stay, window, cx) {
            return;
        }
        let Some(cell) = self.body_cell_at(ev.position) else { return };
        match ev.button {
            MouseButton::Left => {
                let secondary = ev.modifiers.control || ev.modifiers.platform;
                if ev.modifiers.shift
                    && let Some(s) = self.sel
                {
                    let (nr, nc) = (self.n_rows(), self.n_cols());
                    self.set_selection(Some(s.extended_to(cell, nr, nc)), false, cx);
                    self.drag = Some(Drag::Cells);
                    return;
                }
                self.set_selection(Some(Selection::single(cell)), false, cx);
                self.drag = Some(Drag::Cells);
                if secondary {
                    let value = self.value_at(cell.row, cell.col).clone();
                    if self.is_reference_column(cell.col) && !value.is_null() {
                        cx.emit(GridEvent::FollowReference { column: cell.col, value });
                    }
                } else if ev.click_count >= 2 {
                    self.drag = None;
                    self.activate_cursor(false, window, cx);
                }
            }
            MouseButton::Right if !self.current_range().is_some_and(|r| r.contains(cell)) => {
                self.set_selection(Some(Selection::single(cell)), false, cx);
            }
            _ => {}
        }
    }

    pub(crate) fn on_gutter_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.button != MouseButton::Left {
            return;
        }
        window.focus(&self.focus, cx);
        if self.editor.is_some() && !self.commit_edit(After::Stay, window, cx) {
            return;
        }
        let Some(cell) = self.body_cell_at(Point { x: self.body_bounds.get().origin.x, y: ev.position.y })
        else {
            return;
        };
        let (nr, nc) = (self.n_rows(), self.n_cols());
        let next = match self.sel {
            Some(s) if ev.modifiers.shift => {
                let anchor = if s.extent == Extent::Rows { s } else { Selection::row(s.cursor.row, 0) };
                Selection { cursor: Cell::new(cell.row, 0), extent: Extent::Rows, ..anchor }.clamped(nr, nc)
            }
            _ => Selection::row(cell.row, 0),
        };
        self.set_selection(Some(next), false, cx);
        self.drag = Some(Drag::Rows);
    }

    pub(crate) fn on_header_mouse_down(
        &mut self,
        col: usize,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.button != MouseButton::Left || self.n_rows() == 0 {
            return;
        }
        window.focus(&self.focus, cx);
        if self.editor.is_some() && !self.commit_edit(After::Stay, window, cx) {
            return;
        }
        let (nr, nc) = (self.n_rows(), self.n_cols());
        let next = match self.sel {
            Some(s) if ev.modifiers.shift => {
                let anchor = if s.extent == Extent::Cols { s } else { Selection::column(0, s.cursor.col) };
                Selection { cursor: Cell::new(0, col), extent: Extent::Cols, ..anchor }.clamped(nr, nc)
            }
            _ => Selection::column(0, col),
        };
        self.set_selection(Some(next), false, cx);
        self.drag = Some(Drag::Cols);
    }

    pub(crate) fn on_resize_mouse_down(&mut self, col: usize, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left {
            return;
        }
        if ev.click_count >= 2 {
            self.resize = None;
            self.autofit_column(col, cx);
            return;
        }
        self.resize = Some(Resize { col, start_x: f32::from(ev.position.x), start_w: self.cols.width(col) });
    }

    pub(crate) fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pressed = ev.pressed_button == Some(MouseButton::Left);
        if let Some(rz) = &self.resize {
            if pressed {
                let w = rz.start_w + (f32::from(ev.position.x) - rz.start_x);
                let col = rz.col;
                self.cols.set_width(col, w);
                cx.notify();
            } else {
                self.resize = None;
            }
            return;
        }
        if let Some(drag) = self.drag {
            if !pressed {
                self.drag = None;
            } else if let Some(sel) = self.sel {
                self.autoscroll(ev.position);
                let (nr, nc) = (self.n_rows(), self.n_cols());
                if let Some(cell) = self.body_cell_at(ev.position) {
                    let target = match drag {
                        Drag::Cells => cell,
                        Drag::Rows => Cell::new(cell.row, 0),
                        Drag::Cols => Cell::new(0, cell.col),
                    };
                    self.set_selection(Some(sel.extended_to(target, nr, nc)), false, cx);
                }
                cx.notify();
            }
            return;
        }
        let b = self.body_bounds.get();
        let row = b.contains(&ev.position).then(|| self.body_cell_at(ev.position).map(|c| c.row)).flatten();
        if row != self.hover_row {
            self.hover_row = row;
            cx.notify();
        }
    }

    pub(crate) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.drag = None;
        self.resize = None;
    }

    /// Nudge the scroll while dragging a selection past the body edge.
    fn autoscroll(&mut self, pos: Point<Pixels>) {
        let b = self.body_bounds.get();
        let (mut dx, mut dy) = (0.0, 0.0);
        let over = |d: Pixels| f32::from(d) / 3.0 + 4.0;
        if pos.y < b.origin.y {
            dy = -over(b.origin.y - pos.y);
        } else if pos.y > b.origin.y + b.size.height {
            dy = over(pos.y - b.origin.y - b.size.height);
        }
        if pos.x < b.origin.x {
            dx = -over(b.origin.x - pos.x);
        } else if pos.x > b.origin.x + b.size.width {
            dx = over(pos.x - b.origin.x - b.size.width);
        }
        if dx != 0.0 || dy != 0.0 {
            self.scroll.scroll_by(dx, dy);
        }
    }

    pub(crate) fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let d = ev.delta.pixel_delta(px(ROW_H));
        let (mut dx, mut dy) = (-f32::from(d.x), -f32::from(d.y));
        if ev.modifiers.shift && dx == 0.0 {
            std::mem::swap(&mut dx, &mut dy);
        }
        if self.scroll.scroll_by(dx, dy) {
            cx.notify();
        }
        cx.stop_propagation();
    }
}
