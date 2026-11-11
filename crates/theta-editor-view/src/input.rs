//! Platform text input: IME composition, dictation and text committed outside key events.

use crate::view::EditorView;
use gpui_kit::{Bounds, ClipboardItem, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};
use std::ops::Range;
use theta_editor::{Keymap, Mode};

impl EditorView {
    /// May text be inserted right now? Normal / Select / Visual modes only take commands.
    fn text_input_active(&self) -> bool {
        if self.read_only {
            return false;
        }
        match self.editor.keymap() {
            Keymap::Standard => true,
            Keymap::Helix => self.editor.mode() == Mode::Insert || self.editor.prompt().is_some(),
            Keymap::Vim => {
                matches!(self.editor.mode(), Mode::Insert | Mode::Replace) || self.editor.prompt().is_some()
            }
        }
    }

    fn after_text_edit(&mut self, cx: &mut Context<Self>) {
        self.pending_reveal = true;
        self.completion = None;
        self.edited_externally(cx);
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let (s, e) = (self.utf16_to_char(range.start), self.utf16_to_char(range.end));
        *adjusted = Some(self.char_to_utf16(s)..self.char_to_utf16(e));
        Some(self.editor.slice(s..e))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(self.ime_selection())
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.char_to_utf16(r.start)..self.char_to_utf16(r.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.insert_from_platform(&text, window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.text_input_active() {
            return;
        }
        let target = range
            .map(|r| self.utf16_to_char(r.start)..self.utf16_to_char(r.end))
            .or_else(|| self.marked.clone());
        self.marked = None;
        match target {
            Some(r) if self.editor.prompt().is_none() => {
                self.editor.replace_range(r, text);
                self.after_text_edit(cx);
            }
            _ => self.insert_from_platform(text, window, cx),
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Composition lives in the buffer (underlined) until it is committed, as in gpui-kit's input.
        if !self.text_input_active() || self.editor.prompt().is_some() {
            return;
        }
        let target = range
            .map(|r| self.utf16_to_char(r.start)..self.utf16_to_char(r.end))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| {
                let sel = self.editor.primary();
                if self.editor.keymap() == Keymap::Standard {
                    sel.from()..sel.to()
                } else {
                    let c = self.primary_cursor();
                    c..c
                }
            });
        let start = target.start;
        self.editor.replace_range(target, new_text);
        self.marked = (!new_text.is_empty()).then(|| start..start + new_text.chars().count());
        self.after_text_edit(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let idx = self.utf16_to_char(range_utf16.start);
        self.bounds_of_char(idx, cx)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let idx = self.char_at(point, cx)?;
        Some(self.char_to_utf16(idx))
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        !self.read_only
    }
}
